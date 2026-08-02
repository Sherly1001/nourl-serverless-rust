use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlUpsertRequest {
    pub code: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// The public face of a link's owner. Never carries an email or a provider id:
/// the aggregate pipeline strips those before this is built.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerInfo {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
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
    /// RFC3339. Absent on links created before this field existed.
    #[serde(default)]
    pub created_at: Option<String>,
    /// RFC3339, rewritten on every edit. Absent until a link is next written,
    /// so it is not backfilled for existing rows.
    #[serde(default)]
    pub updated_at: Option<String>,
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
    /// Which form field the message belongs to, when the server can say —
    /// lets a UI show it under that input instead of only in a banner.
    /// Absent when the failure is not about one field, and deliberately absent
    /// on a failed login, where naming the wrong half would leak which
    /// usernames exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
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
    // `Url::parse` is happy with a single-label host — `https://google` and
    // `http://localhost` both parse — but a short link is shared with other
    // people, so a host that only resolves on the author's machine (or is a
    // typo for a real domain) is never what was meant.
    match parsed.host() {
        // An address needs no name.
        Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_)) => Ok(()),
        Some(url::Host::Domain(host)) => {
            // One trailing dot is the FQDN root and carries no label.
            let suffix = host
                .strip_suffix('.')
                .unwrap_or(host)
                .rsplit_once('.')
                .map(|(_, suffix)| suffix)
                .ok_or_else(|| {
                    format!("'{host}' is not a full domain name — try something like {host}.com")
                })?;
            // Two characters minimum, starting with a letter. Internationalised
            // domains arrive punycoded (`.рф` is `.xn--p1ai`), so digits and
            // hyphens after the first character have to stay legal.
            let valid = suffix.len() >= 2
                && suffix
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic());
            if valid {
                Ok(())
            } else {
                Err(format!("'{suffix}' is not a valid domain suffix"))
            }
        }
        None => Err("url must include a host".into()),
    }
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
    /// True for the admin nobody promoted — the one seeded directly in the
    /// database. Sent because the sign-in settings are theirs alone, and the
    /// navigation has to know that before it can render, long before any list
    /// of accounts has loaded.
    #[serde(default)]
    pub is_root: bool,
    /// False for accounts that only ever signed in through a provider. The UI
    /// uses it to render "set a password" instead of "change password", and
    /// the server uses the same fact to skip the current-password check.
    #[serde(default)]
    pub has_password: bool,
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
    /// The login handle. Unlike the rest it is validated and must stay unique,
    /// so a change here can come back 400 or 409.
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// `current_password` is optional because an account created through an OAuth
/// provider has no password to prove ownership of — for those, holding a valid
/// session is the whole check. Accounts that do have one must still supply it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    #[serde(default)]
    pub current_password: Option<String>,
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

/// A user as an admin sees them. Never carries a password hash; provider ids
/// are reduced to `providers`, which says *that* an account is linked without
/// exposing the id itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminUserInfo {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    pub is_admin: bool,
    /// How deep in the admin chain: 0 for an admin seeded directly in the
    /// database, and one more for each grant below that. Derived from
    /// `promoted_by` rather than stored, so moving a branch cannot leave a
    /// stale number behind. `None` for anyone who is not an admin.
    #[serde(default)]
    pub admin_level: Option<i32>,
    /// Id of the admin who granted the flag — the parent pointer the tree is
    /// drawn from. `None` for a seeded admin, who nobody promoted, and for
    /// ordinary accounts, who are in no subtree at all.
    #[serde(default)]
    pub promoted_by: Option<String>,
    /// `"github"`, `"google"`, `"facebook"` — whichever are linked.
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub has_password: bool,
    /// RFC3339. Absent on accounts created before the field existed.
    #[serde(default)]
    pub created_at: Option<String>,
    /// How many links this account owns, so the delete confirmation can say
    /// what is about to be orphaned.
    #[serde(default)]
    pub url_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUserListResponse {
    /// Every admin, unpaged and unsearched — the skeleton of the tree. A search
    /// must not drop them, or the tree loses interior nodes and the accounts
    /// below them have nowhere to hang.
    #[serde(default)]
    pub admins: Vec<AdminUserInfo>,
    /// Accounts with no admin flag: paged, sorted and searched as usual. They
    /// belong to no subtree, so they render as a flat bucket under the tree.
    pub items: Vec<AdminUserInfo>,
    /// How many ordinary accounts match the search, for the bucket's count.
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetAdminRequest {
    pub is_admin: bool,
    /// Which admin to hang them under. Defaults to the caller, which is the
    /// ordinary promotion; naming someone else moves them, subtree and all.
    /// Ignored when `is_admin` is false, since a demotion has no parent.
    #[serde(default)]
    pub promoted_by: Option<String>,
    /// What happens to the admins this account promoted. Only read on a
    /// demotion — a promotion or a move leaves the branch where it is.
    #[serde(default)]
    pub orphans: AdminOrphans,
}

/// What becomes of the links an account leaves behind.
///
/// No default: deleting an account is irreversible, and which of these the
/// caller meant is not something to guess at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkDisposition {
    /// Leave them working, unowned, on a deadline. Anyone can claim or edit
    /// them until it runs out.
    Orphan,
    /// Delete them with the account. Every one of them stops resolving
    /// immediately.
    Delete,
}

/// Closing your own account, via `DELETE /api/auth/me`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteAccountRequest {
    pub links: LinkDisposition,
    /// Required when the account has a password — holding the session is not
    /// enough for something this final. Accounts that only ever signed in
    /// through a provider have no password to prove, so for those the session
    /// is the whole check, exactly as in [`ChangePasswordRequest`].
    #[serde(default)]
    pub current_password: Option<String>,
}

/// What happens to the admins an about-to-be-deleted account had promoted.
/// They cannot simply be left behind: `promoted_by` would point at an account
/// that no longer exists, and nothing walking the chain upward could reach them
/// again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminOrphans {
    /// They lose the flag, and so does everyone below them. The safe default:
    /// it never hands anybody a standing they were not given directly.
    #[default]
    Demote,
    /// They keep it, hanging from whoever promoted the deleted account — or
    /// standing as roots of their own if nobody was above it.
    Reparent,
}

/// Query for `DELETE /api/admin/users/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteUserParams {
    #[serde(default)]
    pub orphans: AdminOrphans,
}

/// What a `DELETE /api/admin/users/{id}` did. `orphaned` and `grace_days` are
/// reported so the UI can say what happened to the links rather than leaving
/// the admin to guess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteUserResponse {
    pub id: String,
    pub deleted: bool,
    /// Links that outlived their owner, now unowned and on a deadline.
    #[serde(default)]
    pub orphaned: u64,
    /// Links removed along with the account. Only ever non-zero when the owner
    /// closed their own account and asked for it.
    #[serde(default)]
    pub links_deleted: u64,
    /// How long the orphaned links have left unless someone claims them.
    #[serde(default)]
    pub grace_days: i64,
    /// Admins below the deleted account who lost the flag with them.
    #[serde(default)]
    pub demoted: u64,
    /// Admins the deleted account had promoted who kept the flag and moved up
    /// to its own parent instead. Never non-zero together with `demoted`.
    #[serde(default)]
    pub reparented: u64,
}

/// What changed after a call to `PUT /api/admin/users/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetAdminResponse {
    pub id: String,
    pub is_admin: bool,
    /// Who they now hang under. `None` once demoted.
    #[serde(default)]
    pub promoted_by: Option<String>,
    /// How many accounts lost the flag, counting the target itself: demoting an
    /// admin demotes everyone they promoted, and everyone those admins
    /// promoted. Zero on a promotion or a move.
    #[serde(default)]
    pub demoted: u64,
    /// Admins the demoted account had promoted who kept the flag and moved up
    /// to its own parent instead of losing it.
    #[serde(default)]
    pub reparented: u64,
}

/// One login method as the settings page sees it. The secret itself never
/// leaves the server — only whether one is stored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MethodView {
    pub enabled: bool,
    #[serde(default)]
    pub client_id: Option<String>,
    pub has_secret: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdminSettings {
    pub password: MethodView,
    pub github: MethodView,
    pub google: MethodView,
    pub facebook: MethodView,
}

/// An omitted `client_secret` means "leave the stored one alone", which is how
/// the page can save without ever having seen it. An explicit empty string
/// clears it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MethodUpdate {
    pub enabled: bool,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateSettingsRequest {
    pub password: MethodUpdate,
    pub github: MethodUpdate,
    pub google: MethodUpdate,
    pub facebook: MethodUpdate,
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

    /// `Url::parse` accepts a single-label host, so these all used to pass.
    /// A short link is shared with other people; a host that resolves only on
    /// the author's machine is not a destination.
    #[test]
    fn url_rejects_hosts_that_are_not_full_domain_names() {
        assert!(validate_url("https://google").is_err());
        assert!(validate_url("http://localhost").is_err());
        assert!(validate_url("http://localhost:3000").is_err());
        assert!(validate_url("http://internal-host/path").is_err());
        assert!(validate_url("https://google.").is_err());
        assert!(validate_url("https://x.i").is_err(), "one-letter suffix");
    }

    #[test]
    fn url_accepts_real_hosts_addresses_and_punycode() {
        assert!(validate_url("https://google.com").is_ok());
        assert!(validate_url("https://a.b.co.uk/p?q=1").is_ok());
        assert!(validate_url("https://x.io").is_ok());
        assert!(
            validate_url("https://example.com.").is_ok(),
            "FQDN root dot"
        );
        // Addresses carry no domain name at all.
        assert!(validate_url("http://127.0.0.1:8080").is_ok());
        assert!(validate_url("http://[::1]:8080").is_ok());
        // Internationalised domains are punycoded before we see them.
        assert!(validate_url("https://пример.рф").is_ok());
    }
}
