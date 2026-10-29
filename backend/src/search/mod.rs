pub mod config;
pub mod normalize;
use crate::{auth::Clock, database::Database, models::search::*, youtube::search::SearchClient};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::watch,
    time::{Instant, timeout, timeout_at},
};
#[derive(Clone, Default)]
struct Progress {
    outcome: Option<FlightOutcome>,
    done: bool,
}
pub struct SearchService {
    config: config::SearchConfig,
    clock: Clock,
    provider: SearchClient,
    flights: Mutex<HashMap<String, watch::Sender<Progress>>>,
    #[cfg(test)]
    before_provider: Duration,
    #[cfg(test)]
    block_persistence: bool,
    #[cfg(test)]
    pub(crate) queue_record_delay: Duration,
}

// Drop also runs on task cancellation or panic, so waiters never inherit an orphan flight.
struct FlightCompletion {
    service: Arc<SearchService>,
    query: String,
    tx: watch::Sender<Progress>,
}
impl Drop for FlightCompletion {
    fn drop(&mut self) {
        self.tx.send_modify(|state| {
            if state.outcome.is_none() {
                state.outcome = Some(FlightOutcome::status(
                    DiscoveryStatus::TemporarilyUnavailable,
                ));
            }
            state.done = true;
        });
        self.service.flights.lock().unwrap().remove(&self.query);
    }
}
impl SearchService {
    pub fn new(config: config::SearchConfig, clock: Clock, provider: SearchClient) -> Arc<Self> {
        Arc::new(Self {
            config,
            clock,
            provider,
            flights: Mutex::new(HashMap::new()),
            #[cfg(test)]
            before_provider: Duration::ZERO,
            #[cfg(test)]
            block_persistence: false,
            #[cfg(test)]
            queue_record_delay: Duration::ZERO,
        })
    }
    #[cfg(test)]
    pub fn disabled() -> Arc<Self> {
        Self::new(
            Default::default(),
            Arc::new(|| chrono::Utc::now().timestamp()),
            SearchClient::new().expect("Search client initialization"),
        )
    }
    pub async fn search(
        self: &Arc<Self>,
        db: &Database,
        query: &str,
        more: bool,
        now: i64,
    ) -> Result<SearchResponse, ()> {
        let query = normalize::normalize(query);
        let songs = timeout(Duration::from_secs(1), db.search_catalog(&query, now))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        let outcome = if songs.len() >= 5 && !more {
            FlightOutcome::status(DiscoveryStatus::NotNeeded)
        } else {
            self.discover(db, &query).await
        };
        let mut seen: HashSet<String> = songs.iter().map(|s| s.video_id.clone()).collect();
        let mut results: Vec<SearchResult> = songs
            .into_iter()
            .map(|song| SearchResult {
                song,
                source: Source::Catalog,
            })
            .collect();
        for song in outcome.results {
            if seen.insert(song.video_id.clone()) {
                let source = if outcome.status == DiscoveryStatus::Cached {
                    Source::YoutubeCache
                } else {
                    Source::Youtube
                };
                results.push(SearchResult { song, source });
            }
        }
        Ok(SearchResponse {
            results,
            discovery: Discovery {
                status: outcome.status,
            },
        })
    }
    async fn discover(self: &Arc<Self>, db: &Database, query: &str) -> FlightOutcome {
        let deadline = Instant::now() + Duration::from_millis(4500);
        let mut rx = {
            let mut flights = self.flights.lock().unwrap();
            if let Some(tx) = flights.get(query) {
                tx.subscribe()
            } else {
                let (tx, rx) = watch::channel(Progress::default());
                flights.insert(query.into(), tx.clone());
                let service = self.clone();
                let db = db.clone();
                let query = query.to_owned();
                tokio::spawn(async move {
                    let _completion = FlightCompletion {
                        service: service.clone(),
                        query: query.clone(),
                        tx: tx.clone(),
                    };
                    // This task owns the attempt; cancelling an HTTP caller cannot cancel it.
                    let outcome =
                        timeout_at(deadline, service.run_discovery(&db, &query, &tx, deadline))
                            .await;
                    tx.send_modify(|state| {
                        if let Ok(outcome) = outcome {
                            state.outcome = Some(outcome);
                        }
                        if state.outcome.is_none() {
                            state.outcome = Some(FlightOutcome::status(
                                DiscoveryStatus::TemporarilyUnavailable,
                            ));
                        }
                        state.done = true;
                    });
                });
                rx
            }
        };
        loop {
            let current = rx.borrow_and_update().clone();
            if current.done {
                return current.outcome.unwrap_or_else(|| {
                    FlightOutcome::status(DiscoveryStatus::TemporarilyUnavailable)
                });
            }
            if !matches!(timeout_at(deadline, rx.changed()).await, Ok(Ok(()))) {
                return rx.borrow().outcome.clone().unwrap_or_else(|| {
                    FlightOutcome::status(DiscoveryStatus::TemporarilyUnavailable)
                });
            }
        }
    }
    async fn run_discovery(
        &self,
        db: &Database,
        query: &str,
        tx: &watch::Sender<Progress>,
        deadline: Instant,
    ) -> FlightOutcome {
        let unavailable = || FlightOutcome::status(DiscoveryStatus::TemporarilyUnavailable);
        match db.cached_search(query, (self.clock)()).await {
            Ok(Some(results)) => {
                return FlightOutcome {
                    results,
                    status: DiscoveryStatus::Cached,
                };
            }
            Err(_) => {
                eprintln!("Song search cache unavailable");
                return unavailable();
            }
            _ => {}
        }
        let Some(key) = self.config.api_key.as_deref() else {
            return FlightOutcome::status(DiscoveryStatus::MissingCredentials);
        };
        match db
            .reserve_search((self.clock)(), self.config.daily_limit)
            .await
        {
            Ok(true) => {}
            Ok(false) => return FlightOutcome::status(DiscoveryStatus::DailyLimitReached),
            Err(_) => {
                eprintln!("Song search accounting unavailable");
                return unavailable();
            }
        }
        #[cfg(test)]
        tokio::time::sleep(self.before_provider).await;
        let results = match self.provider.search(query, key).await {
            Ok(results) => results,
            Err(_) => {
                eprintln!("Song discovery temporarily unavailable");
                return unavailable();
            }
        };
        let retrieved = (self.clock)();
        if Instant::now() >= deadline {
            return unavailable();
        }
        let outcome = FlightOutcome {
            results,
            status: DiscoveryStatus::Fresh,
        };
        // Publish first. Deadline and storage failure must preserve even an empty success.
        tx.send_modify(|state| state.outcome = Some(outcome.clone()));
        let write_deadline = deadline.min(Instant::now() + Duration::from_millis(250));
        {
            let persist = async {
                #[cfg(test)]
                if self.block_persistence {
                    std::future::pending::<()>().await;
                }
                db.save_search(query, &outcome.results, retrieved).await
            };
            if Instant::now() < write_deadline
                && !matches!(timeout_at(write_deadline, persist).await, Ok(Ok(())))
            {
                eprintln!("Song discovery persistence unavailable");
            }
        }
        outcome
    }
}
#[cfg(test)]
mod tests;
