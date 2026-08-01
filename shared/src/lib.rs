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

/// Stricter than `validate_code`: no spaces, because a username is typed into
/// a login form where leading and trailing whitespace is invisible and
/// impossible to debug.
pub fn validate_username(username: &str) -> Result<(), String> {
    if username.len() < 3 || username.len() > 32 {
        return Err("username must be 3-32 characters".into());
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err("username may only contain letters, numbers, - and _".into());
    }
    Ok(())
}

pub fn validate_password(password: &str) -> Result<(), String> {
    if password.len() < 8 {
        return Err("password must be at least 8 characters".into());
    }
    Ok(())
}

/// The caller's own account, as returned by `GET /api/auth/me`. Never carries
/// a password hash or provider ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String,
    /// Login handle. Legacy accounts have none, so this falls back to the
    /// display name and then the id — it is for showing, not for matching.
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    pub is_admin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Every field optional: omitting one leaves it untouched rather than
/// clearing it, so a rename does not wipe the avatar.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateProfileRequest {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// Which login methods the server has enabled — drives the login UI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthMethods {
    pub password: bool,
    pub github: bool,
    pub google: bool,
    pub facebook: bool,
}

/// A page of URLs. `total` is the count matching the filter, before
/// `limit`/`skip`, so the UI can render pagination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlListResponse {
    pub items: Vec<UrlEntry>,
    pub total: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_length_bounds() {
        assert!(validate_username("abc").is_ok());
        assert!(validate_username(&"a".repeat(32)).is_ok());
        assert!(validate_username("ab").is_err());
        assert!(validate_username(&"a".repeat(33)).is_err());
    }

    #[test]
    fn username_charset_excludes_spaces() {
        assert!(validate_username("a-b_c1").is_ok());
        assert!(validate_username("bad/name").is_err());
        assert!(validate_username("with space").is_err());
        assert!(validate_username(" leading").is_err());
        assert!(validate_username("trailing ").is_err());
    }

    #[test]
    fn password_minimum_length() {
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password("1234567").is_err());
    }

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
