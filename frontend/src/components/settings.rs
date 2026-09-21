//! Which ways in the site offers, and the credentials behind them.

use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::{AdminSettings, MethodUpdate, MethodView, UpdateSettingsRequest};

use crate::api;
use crate::auth::use_auth;
use crate::toast::use_toasts;

/// An untouched secret is sent as `None`, which the server keeps — the page is
/// never given it, so it has nothing to echo. An empty string retires one.
pub fn to_update(
    enabled: bool,
    client_id: &str,
    secret_draft: &str,
    touched: bool,
) -> MethodUpdate {
    MethodUpdate {
        enabled,
        client_id: Some(client_id.trim().to_string()),
        client_secret: touched.then(|| secret_draft.trim().to_string()),
    }
}

/// What the secret field shows before anyone types in it.
pub fn secret_placeholder(view: &MethodView) -> &'static str {
    if view.has_secret {
        "Saved — type to replace"
    } else {
        "No secret stored"
    }
}

/// Whether a provider would actually work: switched on, and holding both
/// halves of its credential. `view` says whether a secret is already stored,
/// since an untouched field means "keep it" rather than "there is none".
fn usable(update: &MethodUpdate, view: &MethodView) -> bool {
    let has_id = update
        .client_id
        .as_deref()
        .is_some_and(|id| !id.trim().is_empty());
    let has_secret = match update.client_secret.as_deref() {
        // Deliberately cleared.
        Some("") => false,
        Some(_) => true,
        None => view.has_secret,
    };
    update.enabled && has_id && has_secret
}

/// Whether saving would leave nobody a way in. Checked here because the server
/// stores what it is told, and undoing it would need a sign-in.
pub fn locks_everyone_out(request: &UpdateSettingsRequest, stored: &AdminSettings) -> bool {
    if request.password.enabled {
        return false;
    }
    ![
        (&request.github, &stored.github),
        (&request.google, &stored.google),
        (&request.facebook, &stored.facebook),
    ]
    .into_iter()
    .any(|(update, view)| usable(update, view))
}

/// One provider's four fields. `Copy` so every handler can take one without
/// the card having to hand out clones.
#[derive(Clone, Copy)]
struct Draft {
    enabled: RwSignal<bool>,
    client_id: RwSignal<String>,
    secret: RwSignal<String>,
    /// Whether anyone typed in the secret field. Without it there is no way to
    /// tell "left alone" from "cleared", and the two mean opposite things.
    touched: RwSignal<bool>,
}

impl Draft {
    fn new() -> Self {
        Self {
            enabled: RwSignal::new(false),
            client_id: RwSignal::new(String::new()),
            secret: RwSignal::new(String::new()),
            touched: RwSignal::new(false),
        }
    }

    /// Loads what the server sent. The secret box starts empty whatever is
    /// stored — the page never receives it, and a row of dots pretending
    /// otherwise would be a lie.
    fn fill(self, view: &MethodView) {
        self.enabled.set(view.enabled);
        self.client_id
            .set(view.client_id.clone().unwrap_or_default());
        self.secret.set(String::new());
        self.touched.set(false);
    }

    fn to_request(self) -> MethodUpdate {
        to_update(
            self.enabled.get_untracked(),
            &self.client_id.get_untracked(),
            &self.secret.get_untracked(),
            self.touched.get_untracked(),
        )
    }
}

/// One OAuth provider's card. `password` has no credentials, so it gets a
/// simpler card of its own on the page.
#[component]
fn ProviderCard(
    label: &'static str,
    icon: &'static str,
    view: MethodView,
    draft: Draft,
) -> impl IntoView {
    let placeholder = secret_placeholder(&view);
    let id_label = format!("{label} client id");
    let secret_label = format!("{label} client secret");
    view! {
        <div class="border shadow-sm card bg-base-200 border-base-content/10">
            <div class="gap-4 p-6 card-body">
                <div class="flex gap-3 justify-between items-center">
                    <h3 class="flex gap-2 items-center text-xl font-semibold">
                        <span class=format!("{icon} size-5")></span>
                        {label}
                    </h3>
                    <input
                        type="checkbox"
                        class="switch"
                        aria-label=format!("Enable {label}")
                        prop:checked=move || draft.enabled.get()
                        on:change=move |ev| draft.enabled.set(event_target_checked(&ev))
                    />
                </div>

                <form
                    class="contents"
                    autocomplete="off"
                    on:submit=move |ev: leptos::ev::SubmitEvent| ev.prevent_default()
                >
                    <label class="flex flex-col gap-1">
                        <span class="text-sm font-semibold">"Client id"</span>
                        <input
                            class="input"
                            aria-label=id_label
                            prop:value=move || draft.client_id.get()
                            on:input=move |ev| draft.client_id.set(event_target_value(&ev))
                        />
                    </label>

                    <label class="flex flex-col gap-1">
                        <span class="text-sm font-semibold">"Client secret"</span>
                        <input
                            class="input"
                            type="password"
                            autocomplete="off"
                            aria-label=secret_label
                            placeholder=placeholder
                            prop:value=move || draft.secret.get()
                            on:input=move |ev| {
                                draft.secret.set(event_target_value(&ev));
                                draft.touched.set(true);
                            }
                        />
                    </label>
                </form>

                <p class="text-xs text-base-content/50">
                    "Offered on the login page only once it has both halves of its credential."
                </p>
            </div>
        </div>
    }
}

#[component]
pub fn Settings() -> impl IntoView {
    let auth = use_auth();
    let toasts = use_toasts();
    let (loaded, set_loaded) = signal(Option::<AdminSettings>::None);
    let (saving, set_saving) = signal(false);

    let password_enabled = RwSignal::new(true);
    let providers = [
        ("GitHub", "icon-[tabler--brand-github]"),
        ("Google", "icon-[tabler--brand-google]"),
        ("Facebook", "icon-[tabler--brand-facebook]"),
    ];
    // One draft per provider, in the same order as `providers`.
    let drafts = [Draft::new(), Draft::new(), Draft::new()];
    let fill_all = move |settings: &AdminSettings| {
        password_enabled.try_set(settings.password.enabled);
        for (draft, view) in
            drafts
                .iter()
                .zip([&settings.github, &settings.google, &settings.facebook])
        {
            draft.fill(view);
        }
    };

    // Waits for the session: `None` means both in flight and signed out.
    Effect::new(move |_| {
        if !auth.loaded.get() || !auth.is_root() {
            return;
        }
        spawn_local(async move {
            match api::admin_settings().await {
                Ok(settings) => {
                    fill_all(&settings);
                    set_loaded.try_set(Some(settings));
                }
                Err(err) => toasts.error(err.message),
            }
        });
    });

    let save = move |_| {
        let Some(stored) = loaded.get_untracked() else {
            return;
        };
        let request = UpdateSettingsRequest {
            password: MethodUpdate {
                enabled: password_enabled.get_untracked(),
                client_id: None,
                client_secret: None,
            },
            github: drafts[0].to_request(),
            google: drafts[1].to_request(),
            facebook: drafts[2].to_request(),
        };
        if locks_everyone_out(&request, &stored) {
            toasts.error(
                "That would leave no way to sign in. Keep password login on, or finish setting up a provider first.",
            );
            return;
        }
        set_saving.set(true);
        spawn_local(async move {
            match api::save_admin_settings(&request).await {
                Ok(settings) => {
                    // What was just saved cannot be read back.
                    fill_all(&settings);
                    set_loaded.try_set(Some(settings));
                    toasts.success("Settings saved");
                }
                Err(err) => toasts.error(err.message),
            }
            set_saving.try_set(false);
        });
    };

    // Out of the view: leptosfmt reads a `>` in an attribute as a closing bracket.
    let waiting = move || loaded.get().is_none();
    let busy = move || saving.get() || waiting();

    view! {
        <Show
            when=move || auth.is_root()
            fallback=|| {
                view! {
                    <p class="py-16 text-center text-base-content/60">
                        "The sign-in settings belong to the top admin."
                    </p>
                }
            }
        >
            <div class="flex flex-col gap-6 motion-preset-fade motion-duration-500">
                <h2 class="text-3xl font-bold">"Login methods"</h2>

                <div class="border shadow-sm card bg-base-200 border-base-content/10">
                    <div class="flex flex-row gap-3 justify-between items-center p-6 card-body">
                        <h3 class="text-xl font-semibold">"Username and password"</h3>
                        <input
                            type="checkbox"
                            class="switch"
                            aria-label="Enable password login"
                            prop:checked=move || password_enabled.get()
                            on:change=move |ev| password_enabled.set(event_target_checked(&ev))
                        />
                    </div>
                </div>

                <Show when=waiting>
                    <p class="flex gap-2 justify-center items-center py-6 text-base-content/60">
                        <span class="loading loading-spinner loading-sm"></span>
                        "Loading…"
                    </p>
                </Show>

                {move || {
                    loaded
                        .get()
                        .map(|settings| {
                            let views = [
                                settings.github.clone(),
                                settings.google.clone(),
                                settings.facebook.clone(),
                            ];
                            providers
                                .iter()
                                .zip(views)
                                .zip(drafts)
                                .map(|(((label, icon), view), draft)| {
                                    view! {
                                        <ProviderCard label=label icon=icon view=view draft=draft />
                                    }
                                })
                                .collect_view()
                        })
                }}

                <button class="gap-2 self-end btn btn-primary" disabled=busy on:click=save>
                    <Show when=move || saving.get()>
                        <span class="loading loading-spinner loading-sm"></span>
                    </Show>
                    "Save settings"
                </button>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(enabled: bool, id: Option<&str>, has_secret: bool) -> MethodView {
        MethodView {
            enabled,
            client_id: id.map(String::from),
            has_secret,
        }
    }

    #[test]
    fn an_untouched_secret_is_omitted_rather_than_blanked() {
        let untouched = to_update(true, "id", "", false);
        assert_eq!(
            untouched.client_secret, None,
            "omitted keeps the stored one"
        );

        let typed = to_update(true, "id", " new ", true);
        assert_eq!(typed.client_secret.as_deref(), Some("new"));

        // An empty string retires the credential.
        let cleared = to_update(false, "id", "", true);
        assert_eq!(cleared.client_secret.as_deref(), Some(""));
    }

    #[test]
    fn the_placeholder_says_whether_a_secret_exists() {
        assert!(secret_placeholder(&view(true, None, true)).starts_with("Saved"));
        assert!(secret_placeholder(&MethodView::default()).starts_with("No secret"));
    }

    /// Turning password login off is only safe once something else works.
    /// Getting this wrong locks every account out of a site whose settings can
    /// only be changed from inside it.
    #[test]
    fn the_last_way_in_cannot_be_switched_off() {
        let stored = AdminSettings {
            password: view(true, None, false),
            github: view(true, Some("gh"), true),
            ..AdminSettings::default()
        };
        let request = |password: bool, github: MethodUpdate| UpdateSettingsRequest {
            password: MethodUpdate {
                enabled: password,
                ..MethodUpdate::default()
            },
            github,
            ..UpdateSettingsRequest::default()
        };

        // Password on: whatever else happens, there is a way in.
        assert!(!locks_everyone_out(
            &request(true, to_update(false, "", "", false)),
            &stored
        ));
        // Password off, but GitHub is set up and its secret left alone.
        assert!(!locks_everyone_out(
            &request(false, to_update(true, "gh", "", false)),
            &stored
        ));
        // Password off and GitHub's secret cleared in the same save.
        assert!(locks_everyone_out(
            &request(false, to_update(true, "gh", "", true)),
            &stored
        ));
        // Password off and GitHub switched off with it.
        assert!(locks_everyone_out(
            &request(false, to_update(false, "gh", "", false)),
            &stored
        ));
        // A provider with no client id is not a way in either.
        assert!(locks_everyone_out(
            &request(false, to_update(true, "  ", "", false)),
            &stored
        ));
    }
}
