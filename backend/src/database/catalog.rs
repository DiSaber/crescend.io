//! Shared catalog storage, independent of lobby lifecycle.
use super::Database;
use crate::{
    models::search::Snapshot,
    search::normalize::{normalize, terms},
};
use sqlx::{Row, SqliteConnection};
const WEEK: i64 = 604800;
const MONTH: i64 = 2505600;
fn invalid() -> sqlx::Error {
    sqlx::Error::Protocol("Invalid catalog data".into())
}
impl Database {
    pub async fn create_catalog_tables(&self) -> Result<(), sqlx::Error> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_master WHERE name='catalog_metadata_fts'",
        )
        .fetch_one(&mut *tx)
        .await?;
        sqlx::raw_sql(r#"
CREATE TABLE IF NOT EXISTS catalog_songs (
 id INTEGER PRIMARY KEY, video_id TEXT NOT NULL UNIQUE,
 submission_count INTEGER NOT NULL DEFAULT 0 CHECK(submission_count >= 0),
 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS catalog_metadata (
 id INTEGER PRIMARY KEY, song_id INTEGER NOT NULL REFERENCES catalog_songs(id) ON DELETE CASCADE,
 source TEXT NOT NULL CHECK(source IN ('starter','youtube')),
 title TEXT NOT NULL CHECK(length(trim(title))>0), artist TEXT NOT NULL CHECK(length(trim(artist))>0),
 thumbnail_url TEXT, normalized_title TEXT NOT NULL, normalized_artist TEXT NOT NULL,
 fetched_at INTEGER NOT NULL, expires_at INTEGER,
 UNIQUE(song_id,source), CHECK((source='starter' AND expires_at IS NULL) OR (source='youtube' AND expires_at=fetched_at+2505600))) STRICT;
CREATE TABLE IF NOT EXISTS song_search_cache (
 normalized_query TEXT PRIMARY KEY NOT NULL, results_json TEXT NOT NULL,
 fetched_at INTEGER NOT NULL, expires_at INTEGER NOT NULL CHECK(expires_at=fetched_at+604800)) STRICT;
CREATE TABLE IF NOT EXISTS song_search_daily_usage (
 utc_day TEXT PRIMARY KEY NOT NULL, attempts INTEGER NOT NULL CHECK(attempts>=0)) STRICT;
CREATE TABLE IF NOT EXISTS catalog_submission_receipts (
 entry_id TEXT PRIMARY KEY NOT NULL CHECK(length(entry_id)=32 AND entry_id NOT GLOB '*[^0-9a-f]*'),
 song_id INTEGER NOT NULL REFERENCES catalog_songs(id), recorded_at INTEGER NOT NULL) STRICT;
CREATE VIRTUAL TABLE IF NOT EXISTS catalog_metadata_fts USING fts5(
 normalized_title, normalized_artist, content='catalog_metadata', content_rowid='id',
 tokenize='unicode61 remove_diacritics 2', prefix='2 3 4');
CREATE TRIGGER IF NOT EXISTS catalog_metadata_ai AFTER INSERT ON catalog_metadata BEGIN
 INSERT INTO catalog_metadata_fts(rowid,normalized_title,normalized_artist) VALUES(new.id,new.normalized_title,new.normalized_artist); END;
CREATE TRIGGER IF NOT EXISTS catalog_metadata_ad AFTER DELETE ON catalog_metadata BEGIN
 INSERT INTO catalog_metadata_fts(catalog_metadata_fts,rowid,normalized_title,normalized_artist) VALUES('delete',old.id,old.normalized_title,old.normalized_artist); END;
CREATE TRIGGER IF NOT EXISTS catalog_metadata_au AFTER UPDATE ON catalog_metadata BEGIN
 INSERT INTO catalog_metadata_fts(catalog_metadata_fts,rowid,normalized_title,normalized_artist) VALUES('delete',old.id,old.normalized_title,old.normalized_artist);
 INSERT INTO catalog_metadata_fts(rowid,normalized_title,normalized_artist) VALUES(new.id,new.normalized_title,new.normalized_artist); END;
"#).execute(&mut *tx).await?;
        if existing == 0 {
            sqlx::query("INSERT INTO catalog_metadata_fts(catalog_metadata_fts) VALUES('rebuild')")
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }
    async fn upsert_song(
        conn: &mut SqliteConnection,
        s: &Snapshot,
        source: &str,
        now: i64,
    ) -> Result<i64, sqlx::Error> {
        if !s.valid() {
            return Err(invalid());
        }
        let expires = if source == "youtube" {
            Some(now.checked_add(MONTH).ok_or_else(invalid)?)
        } else {
            None
        };
        let id: i64 = sqlx::query_scalar("INSERT INTO catalog_songs(video_id,created_at,updated_at) VALUES(?,?,?) ON CONFLICT(video_id) DO UPDATE SET updated_at=max(updated_at,excluded.updated_at) RETURNING id")
            .bind(&s.video_id).bind(now).bind(now).fetch_one(&mut *conn).await?;
        sqlx::query("INSERT INTO catalog_metadata(song_id,source,title,artist,thumbnail_url,normalized_title,normalized_artist,fetched_at,expires_at) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(song_id,source) DO UPDATE SET title=excluded.title,artist=excluded.artist,thumbnail_url=excluded.thumbnail_url,normalized_title=excluded.normalized_title,normalized_artist=excluded.normalized_artist,fetched_at=excluded.fetched_at,expires_at=excluded.expires_at WHERE excluded.fetched_at>=catalog_metadata.fetched_at")
            .bind(id).bind(source).bind(&s.title).bind(&s.artist).bind(&s.thumbnail_url).bind(normalize(&s.title)).bind(normalize(&s.artist)).bind(now).bind(expires).execute(conn).await?;
        Ok(id)
    }
    pub async fn import_starters(&self, input: &str, now: i64) -> Result<(), sqlx::Error> {
        let entries: Vec<serde_json::Value> = serde_json::from_str(input).map_err(|_| invalid())?;
        let mut seen = std::collections::HashSet::new();
        let mut tx = self.db_pool.begin().await?;
        for mut value in entries {
            if !value.is_object() {
                eprintln!("Invalid starter song skipped");
                continue;
            }
            let Some(id) = value
                .get("video_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
            else {
                eprintln!("Invalid starter song skipped");
                continue;
            };
            value["watch_url"] = format!("https://www.youtube.com/watch?v={id}").into();
            let Ok(s) = serde_json::from_value::<Snapshot>(value) else {
                eprintln!("Invalid starter song skipped");
                continue;
            };
            if !s.valid() {
                eprintln!("Invalid starter song skipped");
                continue;
            }
            if seen.insert(id) {
                Self::upsert_song(&mut tx, &s, "starter", now).await?;
            }
        }
        tx.commit().await
    }
    pub async fn search_catalog(
        &self,
        query: &str,
        now: i64,
    ) -> Result<Vec<Snapshot>, sqlx::Error> {
        let Some(terms) = terms(&normalize(query)) else {
            return Ok(vec![]);
        };
        let rows = sqlx::query("SELECT s.video_id,m.title,m.artist,m.thumbnail_url FROM catalog_metadata_fts JOIN catalog_metadata m ON m.id=catalog_metadata_fts.rowid JOIN catalog_songs s ON s.id=m.song_id WHERE catalog_metadata_fts MATCH ? AND (m.expires_at IS NULL OR m.expires_at>?) AND (m.source='youtube' OR NOT EXISTS(SELECT 1 FROM catalog_metadata p WHERE p.song_id=s.id AND p.source='youtube' AND p.expires_at>?)) ORDER BY bm25(catalog_metadata_fts),s.submission_count DESC,s.video_id LIMIT 10")
            .bind(terms).bind(now).bind(now).fetch_all(&self.db_pool).await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let id: String = r.get("video_id");
                Snapshot {
                    watch_url: format!("https://www.youtube.com/watch?v={id}"),
                    video_id: id,
                    title: r.get("title"),
                    artist: r.get("artist"),
                    thumbnail_url: r.get("thumbnail_url"),
                }
            })
            .collect())
    }
    pub async fn reserve_search(&self, now: i64, limit: i64) -> Result<bool, sqlx::Error> {
        let day = chrono::DateTime::from_timestamp(now, 0)
            .ok_or_else(invalid)?
            .format("%Y-%m-%d")
            .to_string();
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("INSERT INTO song_search_daily_usage(utc_day,attempts) VALUES(?,0) ON CONFLICT DO NOTHING").bind(&day).execute(&mut *tx).await?;
        let changed = sqlx::query("UPDATE song_search_daily_usage SET attempts=attempts+1 WHERE utc_day=? AND attempts<? AND attempts<9223372036854775807").bind(day).bind(limit).execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;
        Ok(changed == 1)
    }
    pub async fn cached_search(
        &self,
        query: &str,
        now: i64,
    ) -> Result<Option<Vec<Snapshot>>, sqlx::Error> {
        let row: Option<(String,i64,i64)> = sqlx::query_as("SELECT results_json,fetched_at,expires_at FROM song_search_cache WHERE normalized_query=? AND expires_at>?").bind(query).bind(now).fetch_optional(&self.db_pool).await?;
        let Some((json, fetched, expires)) = row else {
            return Ok(None);
        };
        if json.len() <= 256 * 1024 && fetched.checked_add(WEEK) == Some(expires) {
            if let Ok(songs) = serde_json::from_str::<Vec<Snapshot>>(&json) {
                if songs.len() <= 10 && songs.iter().all(Snapshot::valid) {
                    return Ok(Some(songs));
                }
            }
        }
        eprintln!("Invalid saved song search ignored");
        Ok(None)
    }
    pub async fn save_search(
        &self,
        query: &str,
        songs: &[Snapshot],
        now: i64,
    ) -> Result<(), sqlx::Error> {
        let expires = now.checked_add(WEEK).ok_or_else(invalid)?;
        let json = serde_json::to_string(songs).map_err(|_| invalid())?;
        if songs.len() > 10 || json.len() > 256 * 1024 || !songs.iter().all(Snapshot::valid) {
            return Err(invalid());
        }
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        for song in songs {
            Self::upsert_song(&mut tx, song, "youtube", now).await?;
        }
        sqlx::query("INSERT INTO song_search_cache(normalized_query,results_json,fetched_at,expires_at) VALUES(?,?,?,?) ON CONFLICT(normalized_query) DO UPDATE SET results_json=excluded.results_json,fetched_at=excluded.fetched_at,expires_at=excluded.expires_at WHERE excluded.fetched_at>song_search_cache.fetched_at").bind(query).bind(json).bind(now).bind(expires).execute(&mut *tx).await?;
        tx.commit().await
    }
    pub async fn record_submission(
        &self,
        entry: &str,
        song: &Snapshot,
        now: i64,
    ) -> Result<(), sqlx::Error> {
        if !song.valid() {
            return Err(invalid());
        }
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let old: Option<String> = sqlx::query_scalar("SELECT s.video_id FROM catalog_submission_receipts r JOIN catalog_songs s ON s.id=r.song_id WHERE r.entry_id=?").bind(entry).fetch_optional(&mut *tx).await?;
        if let Some(id) = old {
            return if id == song.video_id {
                Ok(())
            } else {
                Err(invalid())
            };
        }
        let id = Self::upsert_song(&mut tx, song, "youtube", now).await?;
        sqlx::query(
            "INSERT INTO catalog_submission_receipts(entry_id,song_id,recorded_at) VALUES(?,?,?)",
        )
        .bind(entry)
        .bind(id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let changed = sqlx::query("UPDATE catalog_songs SET submission_count=submission_count+1 WHERE id=? AND submission_count<9223372036854775807").bind(id).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(invalid());
        }
        tx.commit().await
    }
    pub async fn cleanup_catalog(&self, now: i64) -> Result<(), sqlx::Error> {
        let mut tx = self.db_pool.begin().await?;
        sqlx::query("DELETE FROM catalog_metadata WHERE expires_at<=?")
            .bind(now)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM song_search_cache WHERE expires_at<=?")
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }
}
