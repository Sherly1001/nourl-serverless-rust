#[derive(Clone, Debug)]
pub struct Config {
    pub mongo_url: String,
    pub db_name: String,
    pub port: u16,
    /// Explicit override for the not-found redirect target. When `None`, the
    /// redirect falls back to the request's own host (`https://{host}`).
    pub notfound_fallback_url: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let _ = dotenvy::dotenv();
        Ok(Self {
            mongo_url: std::env::var("MONGO_URL")
                .map_err(|_| "MONGO_URL environment variable is required".to_string())?,
            db_name: std::env::var("MONGO_DB").unwrap_or_else(|_| "nourl".into()),
            port: std::env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(9669),
            notfound_fallback_url: std::env::var("NOTFOUND_FALLBACK_URL")
                .ok()
                .filter(|v| !v.is_empty()),
        })
    }
}
