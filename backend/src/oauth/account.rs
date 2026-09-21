//! What to do with a profile once the provider has vouched for it.

use shared::{USERNAME_MAX, USERNAME_MIN};

use super::{Profile, ProviderKind};

/// Long past the point where a human would pick something else.
const MAX_CANDIDATES: usize = 50;

/// What the callback should do with a profile. Every input is already looked
/// up, so the rule itself stays a pure function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The identity is already on this account.
    SignIn(String),
    /// Put the identity on this account, then sign in.
    AttachTo(String),
    /// Nobody holds it and nobody is waiting for it: make an account.
    Create,
    /// The identity belongs to an account other than the one asking to link it.
    Taken,
}

/// `session` is re-read rather than trusted from the flow, and `email_match`
/// is `Some` only when the provider verified the address and exactly one
/// account holds it.
pub fn decide(
    existing: Option<&str>,
    session: Option<&str>,
    link: bool,
    email_match: Option<&str>,
) -> Decision {
    match (existing, session, link) {
        // Known identity, and the session — if any — is a different account.
        (Some(owner), Some(me), true) if owner != me => Decision::Taken,
        (Some(owner), _, _) => Decision::SignIn(owner.to_string()),
        // Unknown identity, and a live session that asked to link it.
        (None, Some(me), true) => Decision::AttachTo(me.to_string()),
        // A verified email is the only other way into an existing account.
        (None, _, _) => match email_match {
            Some(owner) => Decision::AttachTo(owner.to_string()),
            None => Decision::Create,
        },
    }
}

/// The derived name, then `-2`, `-3`; the caller takes the first one free.
/// A provider hands out anything at all, so it is sanitised rather than
/// rejected — rejecting means a form in the middle of a sign-in.
pub fn username_candidates(profile: &Profile, kind: ProviderKind) -> Vec<String> {
    let seed = profile
        .username
        .as_deref()
        .or(profile.display_name.as_deref())
        .unwrap_or(kind.as_str());
    let mut base = sanitise(seed);
    if base.len() < USERNAME_MIN {
        base = sanitise(&format!("{base}-{}", kind.as_str()));
    }
    (1..=MAX_CANDIDATES)
        .map(|n| {
            let suffix = if n == 1 {
                String::new()
            } else {
                format!("-{n}")
            };
            let mut candidate: String = base.chars().take(USERNAME_MAX - suffix.len()).collect();
            // The cut can land on a separator, reading as `some-name--2`.
            if candidate.ends_with('-') {
                candidate.pop();
            }
            candidate.push_str(&suffix);
            candidate
        })
        .collect()
}

/// The charset [`shared::validate_username`] accepts, with no separator runs.
fn sanitise(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let mapped = match ch {
            'a'..='z' | '0'..='9' | '_' => ch,
            'A'..='Z' => ch.to_ascii_lowercase(),
            _ => '-',
        };
        if mapped == '-' && out.ends_with('-') {
            continue;
        }
        out.push(mapped);
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_identity_signs_in_and_a_stranger_is_created() {
        assert_eq!(
            decide(Some("existing"), None, false, None),
            Decision::SignIn("existing".into())
        );
        assert_eq!(decide(None, None, false, None), Decision::Create);
    }

    #[test]
    fn a_link_flow_attaches_to_the_session_and_refuses_a_taken_identity() {
        // Signed in, identity unknown: attach it here.
        assert_eq!(
            decide(None, Some("me"), true, None),
            Decision::AttachTo("me".into())
        );
        // Attaching would hand one account's sign-in to another.
        assert_eq!(
            decide(Some("someone-else"), Some("me"), true, None),
            Decision::Taken
        );
        // Already mine: nothing to do but sign in.
        assert_eq!(
            decide(Some("me"), Some("me"), true, None),
            Decision::SignIn("me".into())
        );
        // `link` but no session: fall back rather than attach to nobody.
        assert_eq!(decide(None, None, true, None), Decision::Create);
    }

    #[test]
    fn a_verified_email_joins_an_existing_account_and_an_unverified_one_does_not() {
        // Only ever `Some` when the provider verified the address.
        assert_eq!(
            decide(None, None, false, Some("by-email")),
            Decision::AttachTo("by-email".into())
        );
        // A known identity always wins: it is the stronger claim.
        assert_eq!(
            decide(Some("known"), None, false, Some("by-email")),
            Decision::SignIn("known".into())
        );
    }

    /// A signed-in browser that did *not* ask to link must not silently have
    /// somebody else's identity bolted onto the account it happens to hold.
    #[test]
    fn a_session_without_the_link_flag_claims_nothing() {
        assert_eq!(decide(None, Some("me"), false, None), Decision::Create);
        assert_eq!(
            decide(Some("someone-else"), Some("me"), false, None),
            Decision::SignIn("someone-else".into())
        );
    }

    #[test]
    fn a_username_is_derived_from_the_handle_and_suffixed_until_free() {
        let profile = |username: Option<&str>, display: Option<&str>| Profile {
            id: "1".into(),
            username: username.map(String::from),
            display_name: display.map(String::from),
            ..Profile::default()
        };

        let candidates =
            username_candidates(&profile(Some("Octo Cat"), None), ProviderKind::Github);
        assert_eq!(candidates[0], "octo-cat", "spaces and case are not allowed");
        assert_eq!(candidates[1], "octo-cat-2");
        assert_eq!(candidates[2], "octo-cat-3");

        // No handle: fall back to the display name, then to the provider.
        assert_eq!(
            username_candidates(&profile(None, Some("A Person!")), ProviderKind::Google)[0],
            "a-person"
        );
        assert_eq!(
            username_candidates(&profile(None, None), ProviderKind::Facebook)[0],
            "facebook"
        );

        // Under three characters is not a legal username, so pad it.
        assert_eq!(
            username_candidates(&profile(Some("ab"), None), ProviderKind::Github)[0],
            "ab-github"
        );

        // And nothing may exceed the 32-character cap, suffix included.
        let long = "x".repeat(40);
        for candidate in username_candidates(&profile(Some(&long), None), ProviderKind::Github) {
            assert!(candidate.len() <= 32, "{candidate} is too long");
        }
    }

    /// Whatever a provider hands over, every candidate has to be a name the
    /// signup form would have accepted: an account is created from one of these
    /// without anybody validating it by hand.
    #[test]
    fn every_candidate_is_a_legal_username() {
        let long = "x".repeat(40);
        let seeds = [
            "Octo Cat",
            "!!!",
            "",
            "   ",
            "-leading-and-trailing-",
            "ünïcødé",
            "a",
            "_",
            &long,
            "many---separators",
        ];
        for seed in seeds {
            for kind in ProviderKind::ALL {
                let profile = Profile {
                    username: Some(seed.to_string()),
                    ..Profile::default()
                };
                for candidate in username_candidates(&profile, kind) {
                    shared::validate_username(&candidate)
                        .unwrap_or_else(|err| panic!("{seed:?} produced {candidate:?}: {err}"));
                }
            }
        }
    }

    /// Truncating a long name must not leave a separator pressed against the
    /// suffix (`some-name--2`).
    #[test]
    fn a_truncated_name_does_not_end_on_a_separator() {
        let profile = Profile {
            // Long enough to be cut, with the separator sitting exactly on the
            // 32-character cap.
            username: Some(format!("{}-tail", "x".repeat(31))),
            ..Profile::default()
        };
        for candidate in username_candidates(&profile, ProviderKind::Github) {
            assert!(!candidate.contains("--"), "{candidate} has a doubled dash");
            assert!(!candidate.ends_with('-'), "{candidate} ends on a separator");
        }
    }

    #[test]
    fn a_new_account_carries_the_profile_it_was_born_from() {
        let profile = Profile {
            id: "123".into(),
            username: Some("octocat".into()),
            display_name: Some("Octo Cat".into()),
            email: Some("octo@example.com".into()),
            email_verified: true,
            avatar_url: Some("https://example.com/a.png".into()),
        };
        let new = crate::users::NewUser::from_profile("octo-cat".into(), &profile);
        assert_eq!(new.username, "octo-cat");
        assert_eq!(new.display_name.as_deref(), Some("Octo Cat"));
        assert_eq!(new.email.as_deref(), Some("octo@example.com"));
        assert_eq!(new.avatar_url.as_deref(), Some("https://example.com/a.png"));
        // No password: these accounts sign in through the provider until their
        // owner sets one.
        assert!(new.hash_passwd.is_none());
    }
}
