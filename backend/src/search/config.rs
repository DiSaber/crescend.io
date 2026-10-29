/// Deliberately does not implement Debug: the key must never enter diagnostics.
#[derive(Clone)]
pub struct SearchConfig {
    pub api_key: Option<String>,
    pub daily_limit: i64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            daily_limit: 90,
        }
    }
}

impl SearchConfig {
    pub fn from_env() -> Result<Self, &'static str> {
        Self::load(|k| std::env::var(k).ok())
    }
    pub fn load(get: impl Fn(&str) -> Option<String>) -> Result<Self, &'static str> {
        let api_key = get("YOUTUBE_API_KEY")
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty());
        let daily_limit = match get("YOUTUBE_SEARCH_DAILY_LIMIT") {
            None => 90,
            Some(v) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => {
                v.parse::<i64>().map_err(
                    |_| "YOUTUBE_SEARCH_DAILY_LIMIT must be a nonnegative 64-bit integer",
                )?
            }
            Some(_) => {
                return Err("YOUTUBE_SEARCH_DAILY_LIMIT must be a nonnegative 64-bit integer");
            }
        };
        Ok(Self {
            api_key,
            daily_limit,
        })
    }
}
