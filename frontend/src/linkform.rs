//! The checks both places that edit a link make before sending it.

use shared::{validate_code, validate_url};

use crate::datetime::{from_display, is_future, local_offset_minutes, to_rfc3339};

pub fn check_code(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        return Some("Code is required".into());
    }
    validate_code(value).err()
}

pub fn check_url(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        return Some("Destination URL is required".into());
    }
    validate_url(value).err()
}

/// The stamp to send, `None` for no expiry. Text that parses to nothing is an
/// error rather than a silent "no expiry", which would drop a typed deadline.
pub fn check_expiry(shown: &str) -> Result<Option<String>, String> {
    if shown.trim().is_empty() {
        return Ok(None);
    }
    let stamp = from_display(shown)
        .and_then(|local| to_rfc3339(&local, local_offset_minutes()))
        .ok_or_else(|| "Expiry is not a valid date and time".to_string())?;
    if !is_future(&stamp) {
        return Err("Expiry must be in the future".into());
    }
    Ok(Some(stamp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_errors_come_from_shared_validators() {
        assert!(check_code("bad/code").is_some());
        assert!(check_url("ftp://x.com").is_some());
    }

    #[test]
    fn valid_input_has_no_error() {
        assert!(check_code("my-code").is_none());
        assert!(check_url("https://example.com").is_none());
    }

    /// Text that parses to nothing must be an error, never a silent "no
    /// expiry" — that would drop a deadline the user asked for.
    #[test]
    fn unparseable_expiry_text_is_refused_but_an_empty_field_is_not() {
        assert!(check_expiry("").unwrap().is_none());
        assert!(check_expiry("   ").unwrap().is_none());
        assert!(check_expiry("nonsense").is_err());
        assert!(check_expiry("2026/13/01 00:00").is_err());
        assert!(check_expiry("2026/08/20").is_err());
    }
}
