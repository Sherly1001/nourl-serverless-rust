use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlUpsertRequest {
    pub code: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerInfo {
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlEntry {
    pub code: String,
    pub url: String,
    #[serde(default)]
    pub owner: Option<OwnerInfo>,
    #[serde(default)]
    pub hits: i64,
    #[serde(default)]
    pub last_hit_at: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteResponse {
    pub code: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub error: ApiErrorBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
}

pub fn validate_code(code: &str) -> Result<(), String> {
    if code.is_empty() || code.len() > 64 {
        return Err("code must be 1-64 characters".into());
    }
    if !code
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' '))
    {
        return Err("code may only contain letters, numbers, spaces, - and _".into());
    }
    Ok(())
}

pub fn validate_url(raw: &str) -> Result<(), String> {
    if raw.len() > 2048 {
        return Err("url must be at most 2048 characters".into());
    }
    let parsed = url::Url::parse(raw).map_err(|_| "url is not a valid URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("url must start with http:// or https://".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_accepts_allowed_charset() {
        assert!(validate_code("abc-DEF_09 x").is_ok());
    }

    #[test]
    fn code_rejects_bad_chars_empty_and_long() {
        assert!(validate_code("héllo").is_err());
        assert!(validate_code("a/b").is_err());
        assert!(validate_code("").is_err());
        assert!(validate_code(&"a".repeat(65)).is_err());
        assert!(validate_code(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn url_requires_http_scheme_and_length() {
        assert!(validate_url("https://example.com/x?y=1").is_ok());
        assert!(validate_url("http://example.com").is_ok());
        assert!(validate_url("ftp://example.com").is_err());
        assert!(validate_url("not a url").is_err());
        let long = format!("https://e.com/{}", "a".repeat(2048));
        assert!(validate_url(&long).is_err());
    }
}
