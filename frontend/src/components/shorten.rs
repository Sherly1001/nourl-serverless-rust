use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::{UrlEntry, UrlUpsertRequest};

use crate::api;
use crate::auth::use_auth;
use crate::clipboard::{copy, origin};
use crate::components::confirm::ConfirmDialog;
use crate::components::conflict::{ReplacementDetails, other_owner};
use crate::components::datepicker::DateTimePicker;
use crate::linkform::{check_code, check_expiry, check_url};
use crate::ui::input_class;

#[component]
pub fn Shorten() -> impl IntoView {
    let (code, set_code) = signal(String::new());
    let (url, set_url) = signal(String::new());
    // Typed in, left, or submitted: not open covered in red.
    let (code_touched, set_code_touched) = signal(false);
    let (url_touched, set_url_touched) = signal(false);
    let (code_server_error, set_code_server_error) = signal(Option::<String>::None);
    let (result, set_result) = signal(Option::<UrlEntry>::None);
    let (busy, set_busy) = signal(false);
    let (expiry, set_expiry) = signal(String::new());
    let (expiry_touched, set_expiry_touched) = signal(false);
    let (copied, set_copied) = signal(false);

    let code_error = Memo::new(move |_| {
        code_server_error.get().or_else(|| {
            code_touched
                .get()
                .then(|| check_code(&code.get()))
                .flatten()
        })
    });
    let url_error = Memo::new(move |_| url_touched.get().then(|| check_url(&url.get())).flatten());
    // Waits to be left: half of a date is not yet a mistake.
    let expiry_error = Memo::new(move |_| {
        expiry_touched
            .get()
            .then(|| check_expiry(&expiry.get()).err())
            .flatten()
    });

    // Submitting is pointless while a field is empty or showing an error.
    let can_submit = Memo::new(move |_| {
        !busy.get()
            && code_error.get().is_none()
            && url_error.get().is_none()
            && expiry_error.get().is_none()
            && check_code(&code.get()).is_none()
            && check_url(&url.get()).is_none()
            && check_expiry(&expiry.get()).is_ok()
    });

    // The link a create collided with, held while the dialog asks about it.
    let (conflict, set_conflict) = signal(None::<UrlEntry>);
    let auth = use_auth();
    let reset_hits_toggle = RwSignal::new(false);
    let claim_owner = RwSignal::new(false);
    let (reset_hits, set_reset_hits) = (
        reset_hits_toggle.read_only(),
        reset_hits_toggle.write_only(),
    );
    let confirm_open = RwSignal::new(false);

    let reset = move || {
        set_code.set(String::new());
        set_url.set(String::new());
        set_code_touched.set(false);
        set_url_touched.set(false);
        set_code_server_error.set(None);
        set_expiry.set(String::new());
        set_expiry_touched.set(false);
        set_result.set(None);
        set_copied.set(false);
    };

    // `overwrite` is true only on a second attempt, after the dialog.
    let send = move |overwrite: bool| {
        let code_value = code.get_untracked();
        let url_value = url.get_untracked();
        let Ok(expires_at) = check_expiry(&expiry.get_untracked()) else {
            return;
        };
        let reset = overwrite && reset_hits.get_untracked();
        let claim = overwrite && claim_owner.get_untracked();

        set_busy.set(true);
        spawn_local(async move {
            let request = UrlUpsertRequest {
                code: code_value,
                url: url_value,
                expires_at,
                overwrite: overwrite.then_some(true),
                reset_hits: reset.then_some(true),
                claim: claim.then_some(true),
            };
            match api::create_url(&request).await {
                Ok(entry) => {
                    set_result.set(Some(entry));
                    set_code_server_error.set(None);
                }
                // A code the caller owns is an offer; anything else is a field error.
                Err(err) => match err.conflict {
                    Some(existing) => {
                        set_reset_hits.set(false);
                        claim_owner.set(false);
                        set_conflict.set(Some(*existing));
                        confirm_open.set(true);
                    }
                    None => set_code_server_error.set(Some(err.message)),
                },
            }
            set_busy.set(false);
        });
    };

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        set_code_touched.set(true);
        set_url_touched.set(true);
        set_expiry_touched.set(true);

        if check_code(&code.get()).is_some()
            || check_url(&url.get()).is_some()
            || check_expiry(&expiry.get()).is_err()
        {
            return;
        }
        send(false);
    };

    let code_input: NodeRef<leptos::html::Input> = NodeRef::new();
    // A frame late: `disabled` is still on the input when the handler runs.
    let focus_code = move || {
        request_animation_frame(move || {
            if let Some(input) = code_input.get_untracked() {
                let _ = input.focus();
            }
        });
    };
    Effect::new(move |_| {
        if let Some(input) = code_input.get() {
            let _ = input.focus();
        }
    });

    // A record of what was made, not a draft: edits here would go nowhere.
    let done = Memo::new(move |_| result.get().is_some());

    view! {
        <ConfirmDialog
            open=confirm_open
            title="Replace this link?"
            message=Signal::derive(move || {
                conflict
                    .get()
                    .map(|existing| match other_owner(&existing, &auth) {
                        Some(name) => format!("/{} belongs to {name}.", existing.code),
                        None => format!("You already use /{}.", existing.code),
                    })
                    .unwrap_or_default()
            })
            confirm_label="Replace"
            confirm_class=Signal::derive(|| "btn-primary".to_string())
            extra=ViewFn::from(move || {
                conflict
                    .get()
                    .map(|existing| {
                        let owner = other_owner(&existing, &auth)
                            .and_then(|_| existing.owner.clone());
                        view! {
                            <ReplacementDetails
                                existing=existing
                                url=url
                                expires=expiry
                                reset_hits=reset_hits_toggle
                                claim=owner.is_some().then_some(claim_owner)
                                other_owner=owner
                            />
                        }
                    })
            })
            on_confirm=Callback::new(move |_| send(true))
        />

        <div class="border shadow-xl card bg-base-200 border-base-content/10 motion-preset-fade motion-duration-500">
            <div class="gap-6 p-8 card-body">
                <h2 class="text-3xl card-title">"Shorten a URL"</h2>

                <form class="flex flex-col gap-3" novalidate on:submit=submit>
                    <div class="w-full">
                        <label class="mb-2 text-lg font-semibold label-text" for="code">
                            "Code"
                        </label>
                        <input
                            id="code"
                            node_ref=code_input
                            class=move || input_class(code_error.get().is_some())
                            placeholder="my-code"
                            autocomplete="off"
                            aria-invalid=move || code_error.get().is_some().to_string()
                            aria-describedby="code-error"
                            disabled=move || done.get()
                            prop:value=code
                            on:input=move |ev| {
                                set_code.set(event_target_value(&ev));
                                set_code_touched.set(true);
                                set_code_server_error.set(None);
                            }
                            on:blur=move |_| set_code_touched.set(true)
                        />
                        <p
                            id="code-error"
                            class="mt-1 text-base min-h-6 text-error motion-preset-slide-down motion-duration-200"
                        >
                            {move || code_error.get().unwrap_or_default()}
                        </p>
                    </div>

                    <div class="w-full">
                        <label class="mb-2 text-lg font-semibold label-text" for="url">
                            "Destination URL"
                        </label>
                        <input
                            id="url"
                            class=move || input_class(url_error.get().is_some())
                            placeholder="https://example.com"
                            autocomplete="off"
                            aria-invalid=move || url_error.get().is_some().to_string()
                            aria-describedby="url-error"
                            disabled=move || done.get()
                            prop:value=url
                            on:input=move |ev| {
                                set_url.set(event_target_value(&ev));
                                set_url_touched.set(true);
                            }
                            on:blur=move |_| set_url_touched.set(true)
                        />
                        <p
                            id="url-error"
                            class="mt-1 text-base min-h-6 text-error motion-preset-slide-down motion-duration-200"
                        >
                            {move || url_error.get().unwrap_or_default()}
                        </p>
                    </div>

                    <div class="w-full">
                        <label class="mb-2 text-lg font-semibold label-text" for="expires">
                            "Expires"
                            <span class="ml-2 text-base font-normal opacity-60">"optional"</span>
                        </label>
                        <DateTimePicker
                            id="expires"
                            value=expiry
                            set_value=set_expiry
                            invalid=Signal::derive(move || expiry_error.get().is_some())
                            disabled=Signal::derive(move || done.get())
                            describedby="expires-error"
                            on_blur=Callback::new(move |_| set_expiry_touched.set(true))
                        />
                        <p
                            id="expires-error"
                            class="mt-1 text-base min-h-6 text-error motion-preset-slide-down motion-duration-200"
                        >
                            {move || expiry_error.get().unwrap_or_default()}
                        </p>
                    </div>

                    <Show
                        when=move || result.get().is_some()
                        fallback=move || {
                            view! {
                                <button
                                    class="h-14 text-xl font-semibold btn btn-primary active:scale-[.98]"
                                    type="submit"
                                    disabled=move || !can_submit.get()
                                >
                                    <Show when=move || busy.get()>
                                        <span class="loading loading-spinner loading-sm"></span>
                                    </Show>
                                    {move || if busy.get() { "Saving…" } else { "Shorten" }}
                                </button>
                            }
                        }
                    >
                        <button
                            class="h-14 text-xl font-semibold btn btn-outline btn-primary active:scale-[.98]"
                            type="button"
                            on:click=move |_| {
                                reset();
                                focus_code();
                            }
                        >
                            <span class="icon-[tabler--plus] size-5"></span>
                            "Create another short link"
                        </button>
                    </Show>
                </form>

                {move || {
                    result
                        .get()
                        .map(|entry| {
                            let short = format!("{}/{}", origin(), entry.code);
                            let href = short.clone();
                            let to_copy = short.clone();
                            view! {
                                <div class="flex gap-3 justify-between items-center alert alert-success motion-preset-slide-up motion-duration-300">
                                    <span class="icon-[tabler--circle-check] size-5 shrink-0"></span>
                                    <a
                                        href=href
                                        target="_blank"
                                        rel="noreferrer"
                                        class="flex-1 font-mono truncate link text-success-content hover:text-success-content focus-visible:text-success-content"
                                    >
                                        {short}
                                    </a>
                                    <button
                                        class="active:scale-95 btn btn-sm btn-outline shrink-0 text-success-content border-success-content/40 hover:bg-success-content/10"
                                        on:click=move |_| {
                                            copy(to_copy.clone());
                                            set_copied.set(true);
                                        }
                                    >
                                        <span class="icon-[tabler--copy] size-4"></span>
                                        {move || if copied.get() { "Copied" } else { "Copy" }}
                                    </button>
                                </div>
                            }
                        })
                }}
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_is_reported_before_format_rules() {
        assert_eq!(check_code("").as_deref(), Some("Code is required"));
        assert_eq!(check_code("   ").as_deref(), Some("Code is required"));
        assert_eq!(
            check_url("").as_deref(),
            Some("Destination URL is required")
        );
    }
}
