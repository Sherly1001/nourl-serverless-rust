use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::{ApiErrorBody, LoginRequest, RegisterRequest, validate_password, validate_username};

use crate::api;
use crate::auth::use_auth;
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
    // Only what could not be pinned to a field: a failed login, a disabled
    // method, a network fault.
    let banner_error = Memo::new(move |_| {
        server_error
            .get()
            .filter(|e| e.field.is_none())
            .map(|e| e.message)
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
                        <input
                            id="password"
                            type="password"
                            class=move || input_class(password_error.get().is_some())
                            autocomplete=move || {
                                if registering.get() { "new-password" } else { "current-password" }
                            }
                            aria-invalid=move || password_error.get().is_some().to_string()
                            aria-describedby="password-error"
                            prop:value=password
                            on:input=move |ev| {
                                set_password.set(event_target_value(&ev));
                                set_touched.set(true);
                                set_server_error.set(None);
                            }
                            on:blur=move |_| set_touched.set(true)
                        />
                        <p
                            id="password-error"
                            class="mt-1 text-base min-h-6 text-error motion-preset-slide-down motion-duration-200"
                        >
                            {move || password_error.get().unwrap_or_default()}
                        </p>
                    </div>

                    <Show when=move || banner_error.get().is_some()>
                        <div class="flex gap-2 items-center alert alert-error motion-preset-slide-down motion-duration-200">
                            <span class="icon-[tabler--alert-circle] size-5 shrink-0"></span>
                            <span>{move || banner_error.get().unwrap_or_default()}</span>
                        </div>
                    </Show>

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
}
