//! Everything an account holder can do to their own account: their profile,
//! their password, the identities they sign in with, and closing it.

use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::{
    AuthMethods, ChangePasswordRequest, DeleteAccountRequest, LinkDisposition,
    UpdateProfileRequest, UserInfo, validate_avatar_url, validate_email, validate_password,
    validate_username,
};

use crate::api;
use crate::auth::use_auth;
use crate::components::avatar::{Avatar, usable_url};
use crate::components::confirm::ConfirmDialog;
use crate::components::password_input::PasswordInput;
use crate::components::tooltip::Tooltip;
use crate::components::users::text::GRACE_DAYS;
use crate::toast::use_toasts;
use crate::ui::input_class;

/// Whether this provider is on the account.
pub fn connected(user: &UserInfo, provider: &str) -> bool {
    user.providers.iter().any(|p| p == provider)
}

/// Whether removing it would still leave a way in. The server refuses the same
/// case with `last_login_method`; this only keeps the button honest.
pub fn can_disconnect(user: &UserInfo, provider: &str) -> bool {
    if !connected(user, provider) {
        return false;
    }
    user.has_password || user.providers.len() > 1
}

/// What to show beside the avatar. Read from the field being typed in rather
/// than from the session, and falling back to the handle the way the navbar
/// does — a display name is optional, and may be being cleared right now.
pub fn shown_name(display_name: &str, username: &str) -> String {
    let trimmed = display_name.trim();
    if trimmed.is_empty() {
        username.to_string()
    } else {
        trimmed.to_string()
    }
}

const PROVIDERS: [(&str, &str, &str); 3] = [
    ("github", "GitHub", "icon-[tabler--brand-github]"),
    ("google", "Google", "icon-[tabler--brand-google]"),
    ("facebook", "Facebook", "icon-[tabler--brand-facebook]"),
];

/// Why the disconnect button is disabled — the same reason the server would
/// give, said before the request rather than after it.
const LAST_WAY_IN: &str = "Set a password first, or connect another provider";

/// What the close dialog warns about. The link count is the part worth
/// knowing: the choice below it is only meaningful once you know how much it
/// applies to.
pub fn close_warning(links: u64) -> String {
    match links {
        0 => "This deletes your account. It cannot be undone.".to_string(),
        1 => "This deletes your account. You own 1 link — choose what happens to it.".to_string(),
        n => format!("This deletes your account. You own {n} links — choose what happens to them."),
    }
}

/// The two answers, worded by what each does to the links rather than by the
/// name of the variant.
pub fn link_choice_label(links: LinkDisposition, grace_days: u32) -> String {
    match links {
        LinkDisposition::Delete => {
            "Delete them with the account — every one stops resolving immediately".to_string()
        }
        LinkDisposition::Orphan => format!(
            "Leave them working, unowned — they expire in {grace_days} days unless someone claims them"
        ),
    }
}

#[component]
pub fn AccountPage() -> impl IntoView {
    let auth = use_auth();
    let toasts = use_toasts();
    let (methods, set_methods) = signal(AuthMethods::default());

    let username = RwSignal::new(String::new());
    let display_name = RwSignal::new(String::new());
    let email = RwSignal::new(String::new());
    let avatar_url = RwSignal::new(String::new());
    let current_password = RwSignal::new(String::new());
    let new_password = RwSignal::new(String::new());
    // Its own field rather than the one above: proving who you are in order to
    // close the account is a different act from changing a password, and
    // borrowing that box would have the dialog read a value the user typed for
    // something else.
    let closing_password = RwSignal::new(String::new());
    let confirm_open = RwSignal::new(false);
    let saving = RwSignal::new(false);
    // What to do with the links. `Orphan` to start with: it is the answer that
    // breaks nothing for whoever is following them, and the destructive one
    // should be reached on purpose rather than by not reading.
    let links = RwSignal::new(LinkDisposition::Orphan);
    // How many there are to decide about. Asked for one row, because only the
    // total is wanted.
    let link_count = RwSignal::new(0u64);

    // Fill the form from the session, and refill it whenever the session
    // changes underneath — a save replaces it.
    Effect::new(move |_| {
        if let Some(me) = auth.user.get() {
            username.set(me.username.clone());
            display_name.set(me.display_name.clone().unwrap_or_default());
            email.set(me.email.clone().unwrap_or_default());
            avatar_url.set(me.avatar_url.clone().unwrap_or_default());
        }
    });
    Effect::new(move |_| {
        spawn_local(async move {
            if let Ok(found) = api::auth_methods().await {
                set_methods.try_set(found);
            }
        });
    });
    Effect::new(move |_| {
        // Only for signed-in callers: the endpoint is authenticated, and the
        // page shows nothing to anyone else.
        if auth.user.get().is_none() {
            return;
        }
        spawn_local(async move {
            if let Ok(page) = api::list_urls(vec![("limit", "1".to_string())], None).await {
                link_count.try_set(page.total);
            }
        });
    });

    let blank_to_none = |value: String| {
        let trimmed = value.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    };

    // The same rules the server applies, so a bad handle is caught before the
    // round trip. Uniqueness is not among them — only the server knows that,
    // and it answers 409 on the field.
    let username_error = Memo::new(move |_| validate_username(&username.get()).err());
    // Both are optional, so blank is not an error — it is the field being
    // cleared, which is what `blank_to_none` sends.
    let email_error = Memo::new(move |_| {
        let typed = email.get();
        let typed = typed.trim();
        (!typed.is_empty())
            .then(|| validate_email(typed).err())
            .flatten()
    });
    let avatar_error = Memo::new(move |_| {
        let typed = avatar_url.get();
        let typed = typed.trim();
        (!typed.is_empty())
            .then(|| validate_avatar_url(typed).err())
            .flatten()
    });
    // Blank is not "no change" for the handle: the field is filled from the
    // session, so an empty box is someone clearing it, which is not a thing.
    let profile_ready = Memo::new(move |_| {
        !saving.get()
            && username_error.get().is_none()
            && email_error.get().is_none()
            && avatar_error.get().is_none()
    });

    let save_profile = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if !profile_ready.get_untracked() {
            return;
        }
        saving.set(true);
        let body = UpdateProfileRequest {
            // Re-sending the current handle is a no-op server-side, so this
            // does not have to work out whether it changed.
            username: blank_to_none(username.get_untracked()),
            display_name: blank_to_none(display_name.get_untracked()),
            email: blank_to_none(email.get_untracked()),
            avatar_url: blank_to_none(avatar_url.get_untracked()),
        };
        spawn_local(async move {
            match api::update_me(&body).await {
                Ok(user) => {
                    auth.user.try_set(Some(user));
                    toasts.success("Profile saved");
                }
                Err(err) => toasts.error(err.message),
            }
            saving.try_set(false);
        });
    };

    // Kept out of the view: leptosfmt reads a comparison's `>` inside an
    // attribute as the element's closing bracket and mangles the markup.
    let has_password = move || auth.user.get().is_some_and(|me| me.has_password);
    let password_heading = move || {
        if has_password() {
            "Change password"
        } else {
            "Set a password"
        }
    };

    // Only once something has been typed: a blank box the user has not reached
    // yet is not an error, it is a box.
    let new_password_error = Memo::new(move |_| {
        let typed = new_password.get();
        (!typed.is_empty())
            .then(|| validate_password(&typed).err())
            .flatten()
    });
    let password_ready = Memo::new(move |_| {
        let typed = new_password.get();
        // The current one is only proof when there is one to prove. The server
        // makes the same distinction, and asking for a password the account has
        // never had would lock a provider-only account out of setting one.
        let proven = !has_password() || !current_password.get().trim().is_empty();
        !typed.is_empty() && new_password_error.get().is_none() && proven
    });

    let save_password = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if !password_ready.get_untracked() {
            return;
        }
        let body = ChangePasswordRequest {
            current_password: blank_to_none(current_password.get_untracked()),
            new_password: new_password.get_untracked(),
        };
        spawn_local(async move {
            match api::change_password(&body).await {
                Ok(user) => {
                    auth.user.try_set(Some(user));
                    current_password.try_set(String::new());
                    new_password.try_set(String::new());
                    toasts.success("Password saved");
                }
                Err(err) => toasts.error(err.message),
            }
        });
    };

    let disconnect = move |provider: &'static str| {
        spawn_local(async move {
            match api::disconnect_provider(provider).await {
                Ok(user) => {
                    auth.user.try_set(Some(user));
                    toasts.success("Disconnected");
                }
                Err(err) => toasts.error(err.message),
            }
        });
    };

    // The server refuses a blank one with a 401 on an account that has a
    // password, so offering the button would only spend a round trip to be told
    // what the page already knows.
    let close_ready = move || !has_password() || !closing_password.get().trim().is_empty();
    // Out of the view for the same reason as `has_password`: a `>` inside an
    // attribute reads as the element's closing bracket.
    let owns_links = move || link_count.get() != 0;

    let close_account = Callback::new(move |()| {
        spawn_local(async move {
            // The links are the account holder's own, so they get the choice an
            // admin acting on someone else never does.
            let body = DeleteAccountRequest {
                links: links.get_untracked(),
                current_password: blank_to_none(closing_password.get_untracked()),
            };
            let result = api::close_account(&body).await;
            closing_password.try_set(String::new());
            match result {
                Ok(_) => {
                    auth.user.try_set(None);
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().set_hash("/");
                    }
                }
                Err(err) => toasts.error(err.message),
            }
        });
    });

    view! {
        <Show
            when=move || auth.user.get().is_some()
            fallback=|| {
                view! {
                    <p class="py-16 text-center text-base-content/60">
                        "Sign in to manage your account."
                    </p>
                }
            }
        >
            <div class="flex flex-col gap-6 motion-preset-fade motion-duration-500">
                <h2 class="text-3xl font-bold">"Account"</h2>

                <div class="border shadow-sm card bg-base-200 border-base-content/10">
                    <div class="gap-4 p-6 card-body">
                        <h3 class="text-xl font-semibold">"Profile"</h3>
                        // A form, so Enter in any field saves rather than doing
                        // nothing — the shape every browser already promises.
                        <form class="contents" novalidate on:submit=save_profile>
                            // Follows the url box as it is typed, so a wrong
                            // address shows itself before it is saved. The
                            // component falls back to the initial on a url that
                            // does not load.
                            <div class="flex gap-4 items-center">
                                {move || {
                                    let name = shown_name(&display_name.get(), &username.get());
                                    view! {
                                        <Avatar
                                            url=usable_url(Some(&avatar_url.get()))
                                            name=name.clone()
                                            size="size-16"
                                        />
                                        <span class="text-lg font-semibold truncate">{name}</span>
                                    }
                                }}
                            </div>
                            <label class="flex flex-col gap-1">
                                <span class="text-sm font-semibold">"Username"</span>
                                <input
                                    class=move || input_class(username_error.get().is_some())
                                    autocomplete="username"
                                    aria-invalid=move || username_error.get().is_some().to_string()
                                    aria-describedby="account-username-error"
                                    prop:value=move || username.get()
                                    on:input=move |ev| username.set(event_target_value(&ev))
                                />
                                <span
                                    id="account-username-error"
                                    class="text-sm min-h-5 text-error"
                                >
                                    {move || username_error.get().unwrap_or_default()}
                                </span>
                            </label>
                            <label class="flex flex-col gap-1">
                                <span class="text-sm font-semibold">"Display name"</span>
                                <input
                                    class=input_class(false)
                                    prop:value=move || display_name.get()
                                    on:input=move |ev| display_name.set(event_target_value(&ev))
                                />
                            </label>
                            <label class="flex flex-col gap-1">
                                <span class="text-sm font-semibold">"Email"</span>
                                <input
                                    class=move || input_class(email_error.get().is_some())
                                    type="email"
                                    autocomplete="email"
                                    aria-invalid=move || email_error.get().is_some().to_string()
                                    aria-describedby="account-email-error"
                                    prop:value=move || email.get()
                                    on:input=move |ev| email.set(event_target_value(&ev))
                                />
                                <span id="account-email-error" class="text-sm min-h-5 text-error">
                                    {move || email_error.get().unwrap_or_default()}
                                </span>
                            </label>
                            <label class="flex flex-col gap-1">
                                <span class="text-sm font-semibold">"Avatar url"</span>
                                <input
                                    class=move || input_class(avatar_error.get().is_some())
                                    aria-invalid=move || avatar_error.get().is_some().to_string()
                                    aria-describedby="account-avatar-error"
                                    placeholder="https://example.com/me.png or data:image/png;base64,…"
                                    prop:value=move || avatar_url.get()
                                    on:input=move |ev| avatar_url.set(event_target_value(&ev))
                                />
                                <span id="account-avatar-error" class="text-sm min-h-5 text-error">
                                    {move || avatar_error.get().unwrap_or_default()}
                                </span>
                            </label>
                            <button
                                class="self-end btn btn-primary"
                                type="submit"
                                disabled=move || !profile_ready.get()
                            >
                                "Save profile"
                            </button>
                        </form>
                    </div>
                </div>

                <div class="border shadow-sm card bg-base-200 border-base-content/10">
                    <div class="gap-4 p-6 card-body">
                        <h3 class="text-xl font-semibold">{password_heading}</h3>
                        <form class="contents" novalidate on:submit=save_password>
                            // Only asked for when there is one to prove; the
                            // server skips the check on an account that has
                            // never had a password.
                            <Show when=has_password>
                                <label class="flex flex-col gap-1">
                                    <span class="text-sm font-semibold">"Current password"</span>
                                    <PasswordInput
                                        value=current_password
                                        autocomplete="current-password"
                                        on_input=Callback::new(move |value| {
                                            current_password.set(value)
                                        })
                                    />
                                </label>
                            </Show>
                            <label class="flex flex-col gap-1">
                                <span class="text-sm font-semibold">"New password"</span>
                                <PasswordInput
                                    value=new_password
                                    invalid=Signal::derive(move || {
                                        new_password_error.get().is_some()
                                    })
                                    describedby="account-password-error"
                                    on_input=Callback::new(move |value| new_password.set(value))
                                />
                                <span
                                    id="account-password-error"
                                    class="text-sm min-h-5 text-error"
                                >
                                    {move || new_password_error.get().unwrap_or_default()}
                                </span>
                            </label>
                            <button
                                class="self-end btn btn-primary"
                                type="submit"
                                disabled=move || !password_ready.get()
                            >
                                "Save password"
                            </button>
                        </form>
                    </div>
                </div>

                <div class="border shadow-sm card bg-base-200 border-base-content/10">
                    <div class="gap-4 p-6 card-body">
                        <h3 class="text-xl font-semibold">"Connected accounts"</h3>
                        {PROVIDERS
                            .into_iter()
                            .map(|(name, label, icon)| {
                                let offered = move || match name {
                                    "github" => methods.get().github,
                                    "google" => methods.get().google,
                                    _ => methods.get().facebook,
                                };
                                let is_connected = move || {
                                    auth.user.get().is_some_and(|me| connected(&me, name))
                                };
                                let removable = move || {
                                    auth.user.get().is_some_and(|me| can_disconnect(&me, name))
                                };
                                // A provider the site does not offer is not a
                                // row at all — unless the account is already on
                                // it, which has to stay visible or there would
                                // be no way to disconnect what an admin has
                                // since switched off.
                                view! {
                                    <Show when=move || offered() || is_connected()>
                                        <div class="flex gap-3 justify-between items-center">
                                            <span class="flex gap-2 items-center">
                                                <span class=format!("{icon} size-5")></span>
                                                {label}
                                            </span>
                                            <Show
                                                when=is_connected
                                                fallback=move || {
                                                    view! {
                                                        // Nothing to offer for a provider the
                                                        // site has switched off: the button
                                                        // would only lead to `oauth_disabled`.
                                                        <Show when=offered>
                                                            <a href=api::oauth_start(name) class="btn btn-soft btn-sm">
                                                                "Connect"
                                                            </a>
                                                        </Show>
                                                    }
                                                }
                                            >
                                                <Show
                                                    when=removable
                                                    fallback=move || {
                                                        view! {
                                                            // Wrapped only while it is refused:
                                                            // a bubble on a button that works
                                                            // explains nothing.
                                                            <Tooltip
                                                                text=LAST_WAY_IN
                                                                class="inline-flex"
                                                                only_when_clipped=false
                                                            >
                                                                <button class="btn btn-error btn-soft btn-sm" disabled=true>
                                                                    "Disconnect"
                                                                </button>
                                                            </Tooltip>
                                                        }
                                                    }
                                                >
                                                    <button
                                                        class="btn btn-error btn-soft btn-sm"
                                                        on:click=move |_| disconnect(name)
                                                    >
                                                        "Disconnect"
                                                    </button>
                                                </Show>
                                            </Show>
                                        </div>
                                    </Show>
                                }
                            })
                            .collect_view()}
                    </div>
                </div>

                <div class="border shadow-sm card bg-base-200 border-error/40">
                    <div class="gap-3 p-6 card-body">
                        <h3 class="text-xl font-semibold text-error">"Close account"</h3>
                        <p class="text-sm text-base-content/70">
                            "You choose what happens to your links. This cannot be undone."
                        </p>
                        <button
                            class="self-end btn btn-error"
                            on:click=move |_| confirm_open.set(true)
                        >
                            "Close account"
                        </button>
                    </div>
                </div>
            </div>

            <ConfirmDialog
                open=confirm_open
                title="Close account"
                message=Signal::derive(move || close_warning(link_count.get()))
                confirm_label="Close account"
                confirm_disabled=Signal::derive(move || !close_ready())
                extra=move || {
                    view! {
                        // Only worth asking when there is something to decide
                        // about. With no links either answer does the same
                        // thing, and a radio pair that changes nothing is a
                        // question the reader has to work out is pointless.
                        <Show when=owns_links>
                            <fieldset class="flex flex-col gap-2 mb-6">
                                <legend class="mb-1 text-sm font-semibold">"Your links"</legend>
                                {[LinkDisposition::Orphan, LinkDisposition::Delete]
                                    .into_iter()
                                    .map(|choice| {
                                        view! {
                                            <label class="flex gap-3 items-start text-sm cursor-pointer">
                                                // No nudge: `radio-sm` is 20px
                                                // and `text-sm` has a 20px line
                                                // box, so `items-start` already
                                                // lands it on the first line. A
                                                // margin here only pushes it off.
                                                <input
                                                    type="radio"
                                                    name="closing-links"
                                                    class="radio radio-sm"
                                                    prop:checked=move || links.get() == choice
                                                    on:change=move |_| links.set(choice)
                                                />
                                                <span>{link_choice_label(choice, GRACE_DAYS)}</span>
                                            </label>
                                        }
                                    })
                                    .collect_view()}
                            </fieldset>
                        </Show>
                        <Show when=has_password>
                            // A form, not a bare label: a password field loose in
                            // the document makes browsers warn, and password
                            // managers need one to offer what they have stored.
                            // Enter does nothing here on purpose — this is not a
                            // dialog to answer by accident.
                            <form
                                class="mb-6"
                                on:submit=move |ev: leptos::ev::SubmitEvent| ev.prevent_default()
                            >
                                <label class="flex flex-col gap-1">
                                    <span class="text-sm font-semibold">"Current password"</span>
                                    <PasswordInput
                                        value=closing_password
                                        autocomplete="current-password"
                                        on_input=Callback::new(move |value| {
                                            closing_password.set(value)
                                        })
                                    />
                                </label>
                            </form>
                        </Show>
                    }
                }
                on_confirm=close_account
            />
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(has_password: bool, providers: &[&str]) -> UserInfo {
        UserInfo {
            id: "u1".into(),
            username: "someone".into(),
            display_name: None,
            email: None,
            avatar_url: None,
            is_admin: false,
            is_root: false,
            has_password,
            providers: providers.iter().map(|p| p.to_string()).collect(),
        }
    }

    /// The count is what makes the choice below it mean anything, so it has to
    /// survive the singular/plural seam.
    #[test]
    fn the_close_warning_counts_the_links_at_stake() {
        assert!(!close_warning(0).contains("link"));
        assert!(close_warning(1).contains("1 link —"));
        assert!(close_warning(12).contains("12 links"));
        for links in [0, 1, 12] {
            assert!(close_warning(links).contains("cannot be undone") || links > 0);
        }
    }

    /// Each option has to say what it does to the links. Naming the variant
    /// would tell the reader nothing they can act on.
    #[test]
    fn each_link_choice_says_what_becomes_of_them() {
        let delete = link_choice_label(LinkDisposition::Delete, 7);
        let orphan = link_choice_label(LinkDisposition::Orphan, 7);
        assert!(delete.contains("stops resolving"));
        assert!(orphan.contains("7 days"));
        assert!(orphan.contains("unowned"));
        assert_ne!(delete, orphan);
        // The grace period is configuration, so the wording follows it.
        assert!(link_choice_label(LinkDisposition::Orphan, 30).contains("30 days"));
    }

    #[test]
    fn the_name_beside_the_avatar_falls_back_to_the_handle() {
        assert_eq!(shown_name("Sher Ly", "sher"), "Sher Ly");
        assert_eq!(shown_name("", "sher"), "sher");
        // Mid-edit, the box can hold nothing but spaces.
        assert_eq!(shown_name("   ", "sher"), "sher");
        assert_eq!(shown_name("  Sher Ly  ", "sher"), "Sher Ly");
    }

    #[test]
    fn a_provider_is_shown_as_connected_only_when_it_is_on_the_account() {
        let me = user(true, &["github"]);
        assert!(connected(&me, "github"));
        assert!(!connected(&me, "google"));
    }

    /// The account must not be left with no way in — the same rule the server
    /// enforces, mirrored so the button can explain itself rather than 400.
    #[test]
    fn the_last_way_in_cannot_be_disconnected() {
        // Only a provider, nothing else: it stays.
        assert!(!can_disconnect(&user(false, &["github"]), "github"));
        // A password is a second way in.
        assert!(can_disconnect(&user(true, &["github"]), "github"));
        // So is another provider.
        assert!(can_disconnect(
            &user(false, &["github", "google"]),
            "github"
        ));
        // Nothing to disconnect in the first place.
        assert!(!can_disconnect(&user(true, &["github"]), "google"));
    }
}
