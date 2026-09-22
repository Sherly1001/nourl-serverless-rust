use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlUpsertRequest {
    pub code: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Proceed through a collision with a code the caller owns. Never opens
    /// someone else's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overwrite: Option<bool>,
    /// Restart the counter. Repointing a link does not on its own end its
    /// history, so it is asked separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_hits: Option<bool>,
    /// Take ownership. Writing over someone's link does not take it: an admin
    /// fixing a destination should not acquire it by not reading a dialog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<bool>,
}

/// A link's owner as everyone else sees them: never an email or provider id.
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
    /// RFC3339, rewritten on every edit. Absent until a link is next written.
    #[serde(default)]
    pub updated_at: Option<String>,
    /// Whether the caller may edit or delete this link. Decided by the server,
    /// because the chain that governs it is not in anything the client holds.
    #[serde(default)]
    pub editable: bool,
    /// Whether the caller may take this link off whoever owns it.
    #[serde(default)]
    pub claimable: bool,
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
    /// Which field the message belongs to, so a UI can show it under that
    /// input. Absent when no single field is at fault, and on a failed login,
    /// where naming the wrong half leaks which usernames exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// The link collided with, on the two conflicts where it is the caller's
    /// to see; a 403 over someone else's code stays useless for lookups.
    /// Boxed: this is the error half of every `Result` in both crates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<Box<UrlEntry>>,
    /// Which ids a bulk request refused, and why. Naming only the first would
    /// have the caller fixing them one round trip at a time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<Vec<RejectedId>>,
}

/// What a bulk action does to every link it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UrlBulkAction {
    Delete,
    Claim,
}

/// `POST /api/urls/bulk`. Every code is checked before anything is written and
/// the writes commit together, so a selection is applied whole or refused
/// whole — as on the users endpoint, though links do not cascade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkUrlsRequest {
    pub codes: Vec<String>,
    pub action: UrlBulkAction,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BulkUrlsResponse {
    /// Links the action applied to.
    pub affected: u64,
    /// The links as they now stand, so a page can patch its rows rather than
    /// reload. Empty for a delete, which leaves nothing to show.
    #[serde(default)]
    pub entries: Vec<UrlEntry>,
}

/// One refused id. `code` is what the error would have carried on its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectedId {
    pub id: String,
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
    // `Url::parse` accepts a single-label host; a shared link cannot.
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
            // IDNs arrive punycoded, so digits and hyphens stay legal after the first.
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

/// Public because OAuth derives a username and must not drift from the check.
pub const USERNAME_MIN: usize = 3;
pub const USERNAME_MAX: usize = 32;

/// Stricter than `validate_code`: no spaces, invisible in a login form.
pub fn validate_username(username: &str) -> Result<(), String> {
    if username.len() < USERNAME_MIN || username.len() > USERNAME_MAX {
        return Err(format!(
            "username must be {USERNAME_MIN}-{USERNAME_MAX} characters"
        ));
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err("username may only contain letters, numbers, - and _".into());
    }
    Ok(())
}

/// Longest address RFC 5321 allows on the wire.
pub const EMAIL_MAX: usize = 254;

/// Shape only. Only a confirmation mail can answer the rest.
pub fn validate_email(email: &str) -> Result<(), String> {
    if email.len() > EMAIL_MAX {
        return Err(format!("email must be at most {EMAIL_MAX} characters"));
    }
    let Some((local, domain)) = email.split_once('@') else {
        return Err("email must look like name@example.com".into());
    };
    // A second `@` would put the split in the wrong place.
    let plausible = !local.is_empty()
        && !domain.contains('@')
        && !email.chars().any(char::is_whitespace)
        && domain
            .rsplit_once('.')
            .is_some_and(|(host, suffix)| !host.is_empty() && suffix.len() >= 2);
    if plausible {
        Ok(())
    } else {
        Err("email must look like name@example.com".into())
    }
}

/// The admin list carries one per row, so a large one bloats every response.
pub const AVATAR_DATA_URI_MAX: usize = 200_000;

/// Must be an image: the value is echoed into pages other people load, and
/// `data:text/html` being inert today is not worth depending on. Both
/// encodings, since a file picker produces base64 and inline SVG does not.
fn is_image_data_uri(raw: &str) -> bool {
    let Some(rest) = raw.strip_prefix("data:image/") else {
        return false;
    };
    let Some((meta, payload)) = rest.split_once(',') else {
        return false;
    };
    // `;base64` is the only parameter that changes how the payload is read.
    let subtype = meta.strip_suffix(";base64").unwrap_or(meta);
    !subtype.is_empty() && !subtype.contains(';') && !payload.is_empty()
}

/// Longest an avatar link may be, matching [`validate_url`].
pub const AVATAR_URL_MAX: usize = 2048;

/// Anything an `<img src>` will load. Looser than [`validate_url`]: an avatar
/// is fetched by the page showing it, so localhost and site-relative are fine.
/// Other schemes stay out — this renders into other people's pages.
pub fn validate_avatar_url(raw: &str) -> Result<(), String> {
    if raw.starts_with("data:") {
        if !is_image_data_uri(raw) {
            return Err("an inline avatar must look like data:image/png;base64,…".into());
        }
        if raw.len() > AVATAR_DATA_URI_MAX {
            return Err(format!(
                "an inline avatar must be at most {AVATAR_DATA_URI_MAX} characters"
            ));
        }
        return Ok(());
    }
    if raw.len() > AVATAR_URL_MAX {
        return Err(format!(
            "avatar url must be at most {AVATAR_URL_MAX} characters"
        ));
    }
    if raw.chars().any(char::is_whitespace) {
        return Err("avatar url must not contain spaces".into());
    }
    // Site-relative and protocol-relative; the browser resolves both.
    if raw.starts_with('/') {
        return Ok(());
    }
    let parsed = url::Url::parse(raw).map_err(|_| {
        "avatar must be an image url, a path like /me.png, or an inline data: image".to_string()
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("avatar url must start with http:// or https://".into());
    }
    if parsed.host().is_none() {
        return Err("avatar url must include a host".into());
    }
    Ok(())
}

pub fn validate_password(password: &str) -> Result<(), String> {
    if password.len() < 8 {
        return Err("password must be at least 8 characters".into());
    }
    Ok(())
}

/// The caller's own account. Never a password hash or provider ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String,
    /// For showing, not matching: legacy accounts fall back to name then id.
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    pub is_admin: bool,
    /// The admin nobody promoted. Sent because the navigation needs it before
    /// any list of accounts has loaded.
    #[serde(default)]
    pub is_root: bool,
    /// False for provider-only accounts, which skip the current-password
    /// check and are offered "set" rather than "change".
    #[serde(default)]
    pub has_password: bool,
    /// Connected providers by name — never the ids themselves.
    #[serde(default)]
    pub providers: Vec<String>,
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

/// Omitting a field leaves it alone, so a rename does not wipe the avatar.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateProfileRequest {
    /// Validated and unique, so a change here can come back 400 or 409.
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// `current_password` is optional only for provider-only accounts, which have
/// none to prove; for the rest the session alone is not enough.
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

/// `total` counts what the filter matched, before `limit`/`skip`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlListResponse {
    pub items: Vec<UrlEntry>,
    pub total: u64,
}

/// What `count_only=true` answers: [`UrlListResponse::total`] without the rows
/// or the owner join.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlCountResponse {
    pub total: u64,
}

/// A user as an admin sees them. Never a password hash, and providers are
/// reduced to names so no id is exposed.
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
    /// Depth in the chain, 0 for a seeded admin. Derived from `promoted_by`
    /// rather than stored, so moving a branch leaves no stale number.
    #[serde(default)]
    pub admin_level: Option<i32>,
    /// The parent pointer the tree is drawn from. `None` for a seeded admin
    /// and for ordinary accounts, who are in no subtree.
    #[serde(default)]
    pub promoted_by: Option<String>,
    /// Whichever providers are linked.
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub has_password: bool,
    /// RFC3339. Absent on accounts created before the field existed.
    #[serde(default)]
    pub created_at: Option<String>,
    /// So the delete confirmation can say what is about to be orphaned.
    #[serde(default)]
    pub url_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUserListResponse {
    /// Every admin, unpaged and unsearched: dropping one would leave the
    /// accounts below it nowhere to hang.
    #[serde(default)]
    pub admins: Vec<AdminUserInfo>,
    /// Accounts in no subtree, so a flat bucket under the tree.
    pub items: Vec<AdminUserInfo>,
    /// How many ordinary accounts the search matched.
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetAdminRequest {
    pub is_admin: bool,
    /// Defaults to the caller. Naming someone else moves them, subtree and
    /// all. Ignored on a demotion, which has no parent.
    #[serde(default)]
    pub promoted_by: Option<String>,
    /// Read only on a demotion; a promotion or move leaves the branch alone.
    #[serde(default)]
    pub orphans: AdminOrphans,
}

/// What becomes of the links an account leaves behind. No default: the
/// deletion is irreversible and the choice is not one to guess at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkDisposition {
    /// Unowned and on a deadline; anyone may claim or edit them until it runs out.
    Orphan,
    /// Deleted with the account, stopping immediately.
    Delete,
}

/// Closing your own account, via `DELETE /api/auth/me`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteAccountRequest {
    pub links: LinkDisposition,
    /// Required when there is one: the session alone is not enough for
    /// something this final. As in [`ChangePasswordRequest`].
    #[serde(default)]
    pub current_password: Option<String>,
}

/// What happens to the admins a deleted account promoted. Leaving them would
/// point `promoted_by` at nothing, stranding them outside the chain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminOrphans {
    /// They and everyone below lose it — never grants an undeserved standing.
    #[default]
    Demote,
    /// They keep it under the deleted account's own parent, or become roots.
    Reparent,
}

/// Query for `DELETE /api/admin/users/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteUserParams {
    #[serde(default)]
    pub orphans: AdminOrphans,
}

/// What a `DELETE /api/admin/users/{id}` did, in enough detail for the UI to
/// say what became of the links.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteUserResponse {
    pub id: String,
    pub deleted: bool,
    /// Links that outlived their owner, now unowned and on a deadline.
    #[serde(default)]
    pub orphaned: u64,
    /// Only non-zero when the owner closed their own account and asked.
    #[serde(default)]
    pub links_deleted: u64,
    /// How long the orphaned links have left.
    #[serde(default)]
    pub grace_days: i64,
    /// Admins below it that lost the flag with it.
    #[serde(default)]
    pub demoted: u64,
    /// Admins that kept the flag and moved up instead. Never with `demoted`.
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
    /// Counting the target: demoting cascades down the whole branch. Zero on
    /// a promotion or a move.
    #[serde(default)]
    pub demoted: u64,
    /// Admins that moved up to its parent instead of losing the flag.
    #[serde(default)]
    pub reparented: u64,
}

/// Promoting hangs them under the caller, as the single-account route does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BulkAction {
    Promote,
    Demote,
    Delete,
}

/// `POST /api/admin/users/bulk`. Every id is checked before anything is
/// written, so a selection is applied whole or refused whole.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkUsersRequest {
    pub ids: Vec<String>,
    pub action: BulkAction,
    /// Read on a demotion and a delete, ignored on a promotion.
    #[serde(default)]
    pub orphans: AdminOrphans,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BulkUsersResponse {
    /// Named accounts the action applied to.
    pub affected: u64,
    /// Admins below the named ones, which `affected` already counts.
    #[serde(default)]
    pub demoted: u64,
    /// Admins that kept the flag and moved up instead.
    #[serde(default)]
    pub reparented: u64,
    /// Links left unowned by a delete.
    #[serde(default)]
    pub orphaned: u64,
    /// How long those links have left.
    #[serde(default)]
    pub grace_days: i64,
}

/// A login method as the settings page sees it: whether a secret is stored,
/// never the secret.
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

/// An omitted `client_secret` leaves the stored one alone, which is how the
/// page saves without ever seeing it; an empty string clears it.
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

    /// A host that resolves only on the author's machine is not a destination.
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
        assert!(validate_url("http://127.0.0.1:8080").is_ok());
        assert!(validate_url("http://[::1]:8080").is_ok());
        assert!(validate_url("https://пример.рф").is_ok());
    }

    #[test]
    fn email_rejects_what_is_plainly_not_an_address() {
        assert!(validate_email("sher@example.com").is_ok());
        assert!(validate_email("first.last+tag@mail.example.co.uk").is_ok());

        assert!(validate_email("sher").is_err(), "no @");
        assert!(validate_email("@example.com").is_err(), "no local part");
        assert!(validate_email("sher@example").is_err(), "no suffix");
        assert!(validate_email("sher@.com").is_err(), "no host");
        assert!(validate_email("a@b@example.com").is_err(), "two @");
        assert!(validate_email("sher @example.com").is_err(), "whitespace");
        assert!(validate_email(&format!("{}@example.com", "a".repeat(250))).is_err());
    }

    /// Looser than [`validate_url`] on purpose: a picture is not a destination.
    #[test]
    fn an_avatar_is_anything_an_img_tag_would_load() {
        assert!(validate_avatar_url("https://example.com/me.png").is_ok());
        assert!(validate_avatar_url("http://localhost:3000/me.png").is_ok());
        assert!(validate_avatar_url("http://127.0.0.1:8080/me.png").is_ok());
        assert!(validate_avatar_url("/favicon.png").is_ok());
        assert!(validate_avatar_url("//cdn.example.com/me.png").is_ok());

        assert!(validate_avatar_url("data:image/png;base64,iVBORw0K").is_ok());
        assert!(validate_avatar_url("data:image/svg+xml;base64,PHN2Zz4=").is_ok());
        assert!(validate_avatar_url("data:image/svg+xml,%3Csvg%2F%3E").is_ok());
        assert!(validate_avatar_url("data:image/gif,rawbytes").is_ok());

        assert!(validate_avatar_url("data:text/html,<b>hi</b>").is_err());
        assert!(validate_avatar_url("data:image/png;base64,").is_err());
        assert!(validate_avatar_url("data:image/,x").is_err());
        assert!(validate_avatar_url("javascript:alert(1)").is_err());
        assert!(validate_avatar_url("vbscript:msgbox(1)").is_err());
        assert!(validate_avatar_url("file:///etc/passwd").is_err());
        assert!(validate_avatar_url("not a url").is_err());
        let huge = format!("data:image/png;base64,{}", "A".repeat(AVATAR_DATA_URI_MAX));
        assert!(validate_avatar_url(&huge).is_err());
        assert!(
            validate_avatar_url(&format!("https://x.io/{}", "a".repeat(AVATAR_URL_MAX))).is_err()
        );
    }
}
