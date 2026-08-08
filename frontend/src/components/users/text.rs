//! What the confirmation dialogs say.
//!
//! Kept apart from the page so the wording — singular against plural, the
//! counts, the difference between one account and a selection — can be read
//! and tested on its own.

use shared::AdminUserInfo;

/// Mirrors the server's `ORPHAN_GRACE_DAYS` default, only to word the
/// confirmation before the request is sent. The response reports the real
/// number, which is what the toast repeats back.
pub const GRACE_DAYS: u32 = 7;

/// What the delete dialog warns about. The link count matters: an admin
/// deleting an account needs to know how much is about to be orphaned.
pub fn delete_warning(user: &AdminUserInfo, grace_days: u32) -> String {
    match user.url_count {
        0 => format!("Delete {}? This cannot be undone.", user.username),
        1 => format!(
            "Delete {}? Their 1 link becomes unowned and expires in {grace_days} days unless someone claims it.",
            user.username
        ),
        n => format!(
            "Delete {}? Their {n} links become unowned and expire in {grace_days} days unless someone claims them.",
            user.username
        ),
    }
}

/// What the demote dialog warns about. Demoting cascades, so the count is the
/// whole point: withdrawing one flag can withdraw several.
pub fn demote_warning(user: &AdminUserInfo, below: usize) -> String {
    match below {
        0 => format!("Remove admin from {}?", user.username),
        1 => format!(
            "Remove admin from {}? The 1 admin they promoted loses it too.",
            user.username
        ),
        n => format!(
            "Remove admin from {}? The {n} admins below them lose it too.",
            user.username
        ),
    }
}

/// The same dialog aimed at yourself. Worth its own wording: the server calls
/// this resigning and treats it as a different act — it needs nobody's
/// permission — and "Remove admin from sher?" asked of sher reads as though
/// somebody else were doing it.
pub fn resign_warning(below: usize) -> String {
    match below {
        0 => "Give up your own admin flag? You will lose the admin pages.".to_string(),
        1 => "Give up your own admin flag? The 1 admin you promoted loses it too.".to_string(),
        n => format!("Give up your own admin flag? The {n} admins below you lose it too."),
    }
}

/// What a bulk delete warns about: how many accounts, and how many links they
/// leave behind between them.
pub fn bulk_delete_warning(users: &[AdminUserInfo], grace_days: u32) -> String {
    let links: u64 = users.iter().map(|user| user.url_count).sum();
    let accounts = users.len();
    match links {
        0 => format!("Delete {accounts} accounts? This cannot be undone."),
        n => format!(
            "Delete {accounts} accounts? Their {n} links become unowned and expire in {grace_days} days unless someone claims them.",
        ),
    }
}

/// What a bulk demote warns about. `below` counts everyone the cascade reaches
/// who was not ticked, which is the part that is easy to miss.
pub fn bulk_demote_warning(users: &[AdminUserInfo], below: usize) -> String {
    let accounts = users.len();
    match below {
        0 => format!("Remove admin from {accounts} accounts?"),
        1 => {
            format!("Remove admin from {accounts} accounts? 1 more admin below them loses it too.")
        }
        n => format!(
            "Remove admin from {accounts} accounts? {n} more admins below them lose it too."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tree::fixtures::*;
    use super::*;

    #[test]
    fn the_bulk_warnings_count_accounts_links_and_the_cascade() {
        let mut one = plain("one");
        one.url_count = 2;
        let mut two = plain("two");
        two.url_count = 3;
        let rows = vec![one, two];
        assert!(bulk_delete_warning(&rows, 7).contains("Delete 2 accounts"));
        assert!(bulk_delete_warning(&rows, 7).contains("Their 5 links"));
        assert!(bulk_delete_warning(&[plain("none")], 7).contains("cannot be undone"));

        assert_eq!(
            bulk_demote_warning(&rows, 0),
            "Remove admin from 2 accounts?"
        );
        assert!(bulk_demote_warning(&rows, 1).contains("1 more admin below"));
        assert!(bulk_demote_warning(&rows, 4).contains("4 more admins below"));
    }

    #[test]
    fn the_warnings_count_what_they_will_change() {
        assert_eq!(
            delete_warning(&plain("solo"), 7),
            "Delete solo? This cannot be undone."
        );
        let one = delete_warning(
            &AdminUserInfo {
                url_count: 1,
                ..plain("one")
            },
            7,
        );
        assert!(one.contains("1 link"), "{one}");
        assert!(
            !one.contains("1 links"),
            "singular, not a stray plural: {one}"
        );
        let many = delete_warning(
            &AdminUserInfo {
                url_count: 4,
                ..plain("many")
            },
            7,
        );
        assert!(
            many.contains("4 links") && many.contains("7 days"),
            "{many}"
        );

        // Demoting says how far the cascade reaches, and says nothing extra
        // when it reaches nobody.
        assert_eq!(demote_warning(&plain("leaf"), 0), "Remove admin from leaf?");
        let one = demote_warning(&plain("mid"), 1);
        assert!(one.contains("The 1 admin they promoted"), "{one}");
        assert!(!one.contains("1 admins"), "singular: {one}");
        assert!(demote_warning(&plain("top"), 3).contains("The 3 admins below them"));
    }

    /// Resigning is addressed to the person doing it, so it never names them
    /// in the third person and always says what they are about to lose.
    #[test]
    fn resigning_is_worded_as_your_own_decision() {
        for below in [0, 1, 3] {
            let text = resign_warning(below);
            assert!(text.contains("your own admin flag"), "{text}");
            assert!(!text.contains("them?"), "not about somebody else: {text}");
        }
        assert!(resign_warning(0).contains("lose the admin pages"));
        let one = resign_warning(1);
        assert!(one.contains("The 1 admin you promoted"), "{one}");
        assert!(!one.contains("1 admins"), "singular: {one}");
        assert!(resign_warning(3).contains("The 3 admins below you"));
    }
}
