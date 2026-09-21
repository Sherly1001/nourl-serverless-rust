use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::{
    ApiErrorBody, AuthMethods, LoginRequest, RegisterRequest, validate_password, validate_username,
};

use crate::api;
use crate::auth::use_auth;
use crate::components::password_input::PasswordInput;
use crate::toast::use_toasts;
use crate::ui::input_class;

/// Sends the browser to `#/urls`, the page a signed-in user came here for.
fn go_to_my_urls() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_hash("/urls");
    }
}

/// The server's message when it belongs to `field`. Errors it could not pin to
/// one field (a failed login, a network fault) stay in the banner instead.
fn error_for<'a>(error: &'a Option<ApiErrorBody>, field: &str) -> Option<&'a str> {
    error
        .as_ref()
        .filter(|e| e.field.as_deref() == Some(field))
        .map(|e| e.message.as_str())
}

fn check_username(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        return Some("Username is required".into());
    }
    validate_username(value).err()
}

/// Only the length floor is enforced while signing *in*: an older account may
/// predate a rule, and the server is the one that decides anyway.
fn check_password(value: &str, registering: bool) -> Option<String> {
    if value.is_empty() {
        return Some("Password is required".into());
    }
    if registering {
        return validate_password(value).err();
    }
    None
}

pub struct ProviderButton {
    pub name: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
}

/// The providers worth drawing a button for. `AuthMethods` already reports a
/// provider as available only when it is switched on *and* holds both halves of
/// its credential, so a button here always leads somewhere.
pub fn enabled_providers(methods: &AuthMethods) -> Vec<ProviderButton> {
    [
        (
            methods.github,
            "github",
            "GitHub",
            "icon-[tabler--brand-github]",
        ),
        (
            methods.google,
            "google",
            "Google",
            "icon-[tabler--brand-google]",
        ),
        (
            methods.facebook,
            "facebook",
            "Facebook",
            "icon-[tabler--brand-facebook]",
        ),
    ]
    .into_iter()
    .filter(|(on, ..)| *on)
    .map(|(_, name, label, icon)| ProviderButton { name, label, icon })
    .collect()
}

/// Plain words for the `?error=` the callback redirects with. The provider's
/// own message never reaches here — it is not ours to show — so this is the
/// whole explanation the user gets, and it has to be worth reading.
pub fn oauth_error_message(code: &str) -> &'static str {
    match code {
        "oauth_disabled" => "That sign-in method is switched off.",
        "oauth_denied" => "You cancelled the sign-in.",
        "oauth_state" => "That sign-in took too long or was interrupted. Try again.",
        "oauth_exchange" => "The provider could not confirm who you are. Try again.",
        "oauth_taken" => "Another account is already connected to that login.",
        _ => "Something went wrong with that sign-in.",
    }
}

/// Splits `#/login?error=code` into the route to stay on and the code to
/// report. Separate from the effect that acts on it so the parsing can be
/// tested without a browser.
fn error_in_hash(hash: &str) -> Option<(&str, &str)> {
    let (path, query) = hash.split_once('?')?;
    let code = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("error="))?;
    Some((path, code))
}

#[component]
pub fn Login() -> impl IntoView {
    let auth = use_auth();
    // Nobody signed in belongs on this page. Covers both arriving with a
    // session already in hand and the moment a sign-in succeeds, so the submit
    // handler does not navigate itself.
    Effect::new(move |_| {
        if auth.user.get().is_some() {
            go_to_my_urls();
        }
    });

    // Which buttons to draw. Defaults to nothing rather than to everything, so
    // a failed load shows no button that leads to a switched-off provider.
    let (methods, set_methods) = signal(AuthMethods::default());
    Effect::new(move |_| {
        spawn_local(async move {
            if let Ok(found) = api::auth_methods().await {
                set_methods.try_set(found);
            }
        });
    });

    let (registering, set_registering) = signal(false);
    let (username, set_username) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (touched, set_touched) = signal(false);
    let (server_error, set_server_error) = signal(Option::<ApiErrorBody>::None);
    let (busy, set_busy) = signal(false);

    let username_error = Memo::new(move |_| {
        touched
            .get()
            .then(|| check_username(&username.get()))
            .flatten()
            .or_else(|| error_for(&server_error.get(), "username").map(String::from))
    });
    let password_error = Memo::new(move |_| {
        touched
            .get()
            .then(|| check_password(&password.get(), registering.get()))
            .flatten()
            .or_else(|| error_for(&server_error.get(), "password").map(String::from))
    });
    // Whatever could not be pinned to a field — a failed login, a disabled
    // method, a network fault — is a system-level message, so it goes to the
    // toast stack rather than growing the form.
    let toasts = use_toasts();
    Effect::new(move |_| {
        if let Some(err) = server_error.get().filter(|e| e.field.is_none()) {
            toasts.error(err.message);
        }
    });
    // The callback reports failure through the URL, since it redirects rather
    // than answering. Say it once, then take it back out of the address bar.
    Effect::new(move |_| {
        let Some(window) = web_sys::window() else {
            return;
        };
        let hash = window.location().hash().unwrap_or_default();
        let Some((path, code)) = error_in_hash(&hash) else {
            return;
        };
        toasts.error(oauth_error_message(code).to_string());
        let _ = window.location().set_hash(path);
    });

    let can_submit = Memo::new(move |_| {
        !busy.get()
            && check_username(&username.get()).is_none()
            && check_password(&password.get(), registering.get()).is_none()
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        set_touched.set(true);
        let (name, pass, is_register) = (username.get(), password.get(), registering.get());
        if check_username(&name).is_some() || check_password(&pass, is_register).is_some() {
            return;
        }

        set_busy.set(true);
        set_server_error.set(None);
        spawn_local(async move {
            let result = if is_register {
                api::register(&RegisterRequest {
                    username: name,
                    password: pass,
                })
                .await
            } else {
                api::login(&LoginRequest {
                    username: name,
                    password: pass,
                })
                .await
            };
            match result {
                // The redirect is left to the effect below, which also covers
                // arriving here with a session already in hand.
                Ok(user) => {
                    auth.user.set(Some(user));
                    auth.loaded.set(true);
                }
                Err(err) => set_server_error.set(Some(err)),
            }
            set_busy.set(false);
        });
    };

    let username_input: NodeRef<leptos::html::Input> = NodeRef::new();
    Effect::new(move |_| {
        if let Some(input) = username_input.get() {
            let _ = input.focus();
        }
    });

    view! {
        <div class="border shadow-xl card bg-base-200 border-base-content/10 motion-preset-fade motion-duration-500">
            <div class="gap-6 p-8 card-body">
                <h2 class="text-3xl card-title">
                    {move || if registering.get() { "Create an account" } else { "Sign in" }}
                </h2>

                <Show when=move || methods.get().password>
                    <form class="flex flex-col gap-3" novalidate on:submit=submit>
                        <div class="w-full">
                            <label class="mb-2 text-lg font-semibold label-text" for="username">
                                "Username"
                            </label>
                            <input
                                id="username"
                                node_ref=username_input
                                class=move || input_class(username_error.get().is_some())
                                autocomplete="username"
                                aria-invalid=move || username_error.get().is_some().to_string()
                                aria-describedby="username-error"
                                prop:value=username
                                on:input=move |ev| {
                                    set_username.set(event_target_value(&ev));
                                    set_touched.set(true);
                                    set_server_error.set(None);
                                }
                                on:blur=move |_| set_touched.set(true)
                            />
                            <p
                                id="username-error"
                                class="mt-1 text-base min-h-6 text-error motion-preset-slide-down motion-duration-200"
                            >
                                {move || username_error.get().unwrap_or_default()}
                            </p>
                        </div>

                        <div class="w-full">
                            <label class="mb-2 text-lg font-semibold label-text" for="password">
                                "Password"
                            </label>
                            <PasswordInput
                                id="password"
                                value=password
                                invalid=Signal::derive(move || password_error.get().is_some())
                                describedby="password-error"
                                autocomplete=Signal::derive(move || {
                                    if registering.get() {
                                        "new-password".to_string()
                                    } else {
                                        "current-password".to_string()
                                    }
                                })
                                on_input=Callback::new(move |value| {
                                    set_password.set(value);
                                    set_touched.set(true);
                                    set_server_error.set(None);
                                })
                                on_blur=Callback::new(move |()| set_touched.set(true))
                            />
                            <p
                                id="password-error"
                                class="mt-1 text-base min-h-6 text-error motion-preset-slide-down motion-duration-200"
                            >
                                {move || password_error.get().unwrap_or_default()}
                            </p>
                        </div>

                        <button
                            class="h-14 text-xl font-semibold btn btn-primary active:scale-[.98]"
                            type="submit"
                            disabled=move || !can_submit.get()
                        >
                            <Show when=move || busy.get()>
                                <span class="loading loading-spinner loading-sm"></span>
                            </Show>
                            {move || {
                                match (registering.get(), busy.get()) {
                                    (true, true) => "Creating…",
                                    (true, false) => "Create account",
                                    (false, true) => "Signing in…",
                                    (false, false) => "Sign in",
                                }
                            }}
                        </button>
                    </form>

                    <button
                        class="btn btn-text"
                        type="button"
                        on:click=move |_| {
                            set_registering.update(|r| *r = !*r);
                            set_server_error.set(None);
                            set_touched.set(false);
                        }
                    >
                        {move || {
                            if registering.get() {
                                "Already have an account? Sign in"
                            } else {
                                "Need an account? Create one"
                            }
                        }}
                    </button>
                </Show>

                <Show when=move || !enabled_providers(&methods.get()).is_empty()>
                    <Show when=move || methods.get().password>
                        <div class="text-xs text-center uppercase text-base-content/40">"or"</div>
                    </Show>
                    <div class="flex flex-col gap-2">
                        {move || {
                            enabled_providers(&methods.get())
                                .into_iter()
                                .map(|provider| {
                                    view! {
                                        <a
                                            href=api::oauth_start(provider.name)
                                            class="gap-2 w-full btn btn-soft"
                                        >
                                            <span class=format!("{} size-5", provider.icon)></span>
                                            {format!("Continue with {}", provider.label)}
                                        </a>
                                    }
                                })
                                .collect_view()
                        }}
                    </div>
                </Show>

                <Show when=move || {
                    !methods.get().password && enabled_providers(&methods.get()).is_empty()
                }>
                    <p class="py-6 text-center text-base-content/60">
                        "No sign-in method is switched on. Ask an admin."
                    </p>
                </Show>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_is_reported_before_format_rules() {
        assert_eq!(check_username("").as_deref(), Some("Username is required"));
        assert_eq!(
            check_username("   ").as_deref(),
            Some("Username is required")
        );
        assert_eq!(
            check_password("", true).as_deref(),
            Some("Password is required")
        );
    }

    #[test]
    fn username_rules_come_from_the_shared_validator() {
        assert!(check_username("ab").is_some());
        assert!(check_username("has space").is_some());
        assert!(check_username("good-name_1").is_none());
    }

    /// The length floor is a registration rule. Enforcing it on sign-in would
    /// lock out any account whose password predates the rule.
    #[test]
    fn the_password_floor_applies_only_when_registering() {
        assert!(check_password("short", true).is_some());
        assert!(check_password("short", false).is_none());
        assert!(check_password("longenough", true).is_none());
    }

    fn api_error(field: Option<&str>) -> Option<ApiErrorBody> {
        Some(ApiErrorBody {
            code: "conflict".into(),
            message: "that username is already taken".into(),
            field: field.map(String::from),
            conflict: None,
            rejected: None,
        })
    }

    #[test]
    fn a_server_error_lands_on_the_field_it_names() {
        let taken = api_error(Some("username"));
        assert_eq!(
            error_for(&taken, "username"),
            Some("that username is already taken")
        );
        assert_eq!(error_for(&taken, "password"), None);
    }

    /// A failed login names no field on purpose — saying "wrong password"
    /// would confirm the username exists. Those stay in the banner.
    #[test]
    fn an_unattributed_error_belongs_to_no_field() {
        let anonymous = api_error(None);
        assert_eq!(error_for(&anonymous, "username"), None);
        assert_eq!(error_for(&anonymous, "password"), None);
        assert_eq!(error_for(&None, "username"), None);
    }

    #[test]
    fn only_configured_providers_get_a_button() {
        let methods = AuthMethods {
            password: true,
            github: true,
            google: false,
            facebook: true,
        };
        let names: Vec<&str> = enabled_providers(&methods).iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["github", "facebook"]);
        assert!(enabled_providers(&AuthMethods::default()).is_empty());
    }

    /// Every code `routes::oauth::refuse` can redirect with. A code with no arm
    /// here falls through to the catch-all, which is honest but says nothing
    /// useful, so the list is worth keeping in step by hand.
    #[test]
    fn every_callback_failure_has_words_of_its_own() {
        for code in [
            "oauth_disabled",
            "oauth_denied",
            "oauth_state",
            "oauth_exchange",
            "oauth_taken",
        ] {
            let message = oauth_error_message(code);
            assert!(!message.is_empty(), "{code} has no wording");
            assert!(!message.contains('_'), "{code} leaked its code: {message}");
        }
        // Anything unrecognised still says something honest.
        assert!(!oauth_error_message("something-else").is_empty());
    }

    #[test]
    fn the_error_is_read_out_of_the_hash_and_the_rest_of_it_kept() {
        assert_eq!(
            error_in_hash("#/login?error=oauth_denied"),
            Some(("#/login", "oauth_denied"))
        );
        // Not necessarily the only parameter, nor the first.
        assert_eq!(
            error_in_hash("#/login?from=x&error=oauth_state"),
            Some(("#/login", "oauth_state"))
        );
        for quiet in ["", "#/login", "#/login?from=x", "#/login?errorish=1"] {
            assert_eq!(error_in_hash(quiet), None, "{quiet} should say nothing");
        }
    }
}
