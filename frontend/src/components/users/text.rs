//! What the confirmation dialogs say, kept apart so it can be tested.

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

/// The same dialog aimed at yourself, which the server treats as a different
/// act — and "Remove admin from sher?" asked of sher reads as somebody else.
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

/// What a bulk request did, for the toast that follows it. `below` and `moved`
/// are the cascade — the accounts that were not named but were reached anyway —
/// and go unmentioned when there were none.
pub fn bulk_result(word: &str, affected: u64, below: u64, moved: u64) -> String {
    match (below, moved) {
        (0, 0) => format!("{affected} {word}"),
        (_, kept) if kept > 0 => format!("{affected} {word} ({kept} kept their admin flag)"),
        (lost, _) => format!("{affected} {word} ({lost} more below them lost the flag)"),
    }
}

/// A refused bulk request, worded for a toast. One refused row out of forty is
/// a different thing to untick than thirty-nine, so the count matters.
pub fn bulk_refusal(message: &str, rejected: &[shared::RejectedId]) -> String {
    let Some(first) = rejected.first() else {
        return message.to_string();
    };
    let accounts = match rejected.len() {
        1 => "1 account was".to_string(),
        n => format!("{n} accounts were"),
    };
    format!("{message}: {accounts} refused — {}", first.message)
}

#[cfg(test)]
mod tests {
    use shared::RejectedId;

    use super::super::tree::fixtures::*;
    use super::*;

    /// The counts a bulk request comes back with, worded for a toast.
    #[test]
    fn the_bulk_result_reports_the_cascade_only_when_there_was_one() {
        assert_eq!(bulk_result("promoted", 2, 0, 0), "2 promoted");
        assert_eq!(
            bulk_result("demoted", 2, 3, 0),
            "2 demoted (3 more below them lost the flag)"
        );
        assert_eq!(
            bulk_result("deleted", 1, 0, 2),
            "1 deleted (2 kept their admin flag)"
        );
        assert_eq!(
            bulk_result("demoted", 1, 1, 0),
            "1 demoted (1 more below them lost the flag)"
        );
    }

    /// A refusal names the ids it was about, because the whole selection is
    /// judged as one and the admin has to know which rows to untick.
    #[test]
    fn a_refusal_names_the_accounts_it_was_about() {
        let refused = |ids: &[&str]| {
            ids.iter()
                .map(|id| RejectedId {
                    id: (*id).to_string(),
                    code: "forbidden".into(),
                    message: "not in your part of the chain".into(),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            bulk_refusal("nothing changed", &refused(&["a"])),
            "nothing changed: 1 account was refused — not in your part of the chain"
        );
        assert_eq!(
            bulk_refusal("nothing changed", &refused(&["a", "b"])),
            "nothing changed: 2 accounts were refused — not in your part of the chain"
        );
        // No list to draw on: the message is all there is.
        assert_eq!(bulk_refusal("it broke", &[]), "it broke");
    }

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

        // Says how far the cascade reaches, and nothing when it reaches nobody.
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
