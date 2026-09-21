use leptos::html::{Div, Input};
use leptos::prelude::*;
use shared::{OwnerInfo, UrlEntry};

use crate::components::conflict::ReplacementDetails;
use crate::components::datepicker::DateTimePicker;
use crate::components::tooltip::Tooltip;
use crate::linkform::{check_code, check_expiry, check_url};
use crate::ui::input_class;

/// What a collision says, above the summary of what replacing would change.
fn conflict_message(code: &str, owner: Option<&str>, renaming: bool) -> String {
    match (owner, renaming) {
        (Some(owner), true) => {
            format!("/{code} belongs to {owner}. Moving this link onto it deletes theirs.")
        }
        (Some(owner), false) => format!("/{code} belongs to {owner}."),
        (None, true) => format!("/{code} already exists. Moving this link onto it deletes it."),
        (None, false) => format!("You already use /{code}."),
    }
}

/// The Shorten form in a modal. Not [`crate::components::confirm::ConfirmDialog`],
/// which focuses Cancel; a collision is answered here rather than in a second
/// dialog, so changing the code to a free one stays within reach.
#[component]
pub fn EditLinkDialog(
    open: RwSignal<bool>,
    /// The code the row had when editing began, which the title names.
    #[prop(into)]
    original: Signal<String>,
    code: RwSignal<String>,
    url: RwSignal<String>,
    expiry: RwSignal<String>,
    invalid: RwSignal<Option<&'static str>>,
    #[prop(into)] saving: Signal<bool>,
    /// The link a save collided with, and whether landing on it deletes it.
    #[prop(into)]
    conflict: Signal<Option<(UrlEntry, bool)>>,
    reset_hits: RwSignal<bool>,
    claim: RwSignal<bool>,
    /// The other link's owner, when that is somebody else.
    #[prop(into)]
    other_owner: Signal<Option<OwnerInfo>>,
    /// Carries whether the caller has seen the collision and means it.
    on_save: Callback<bool>,
) -> impl IntoView {
    let card: NodeRef<Div> = NodeRef::new();
    let code_input: NodeRef<Input> = NodeRef::new();
    // Typed in, left, or saved over: not open covered in red.
    let touched = RwSignal::new(false);
    let code_error = Memo::new(move |_| touched.get().then(|| check_code(&code.get())).flatten());
    let url_error = Memo::new(move |_| touched.get().then(|| check_url(&url.get())).flatten());
    let expiry_error = Memo::new(move |_| {
        touched
            .get()
            .then(|| check_expiry(&expiry.get()).err())
            .flatten()
    });
    // So Escape closes the popover without also closing the dialog.
    let popover = RwSignal::new(false);

    let close = move || {
        open.set(false);
        invalid.set(None);
    };
    let colliding = move || conflict.get().is_some();
    let save = move || {
        touched.set(true);
        on_save.run(conflict.get_untracked().is_some());
    };

    // Where an edit starts; otherwise the keyboard route opens with a Tab.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        request_animation_frame(move || {
            if let Some(input) = code_input.get_untracked() {
                let _ = input.focus();
                input.select();
            }
        });
    });

    view! {
        <Show when=move || open.get()>
            <div
                class="flex fixed inset-0 z-50 justify-center items-center p-4 bg-black/50 motion-preset-fade motion-duration-200"
                role="dialog"
                aria-modal="true"
                on:click=move |ev| {
                    let backdrop = ev.target().map(wasm_bindgen::JsValue::from)
                        == ev.current_target().map(wasm_bindgen::JsValue::from);
                    if backdrop {
                        close();
                    }
                }
                on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                    if ev.key() != "Escape" {
                        return;
                    }
                    if !popover.get_untracked() {
                        close();
                    }
                }
            >
                <div
                    node_ref=card
                    class="flex flex-col gap-6 p-8 w-full max-w-2xl rounded-lg border shadow-xl bg-base-100 border-base-content/10 motion-preset-slide-down motion-duration-200"
                >
                    <h3 class="font-mono text-3xl font-semibold">
                        <Tooltip text=format!(
                            "Edit /{}",
                            original.get(),
                        )>{move || format!("Edit /{}", original.get())}</Tooltip>
                    </h3>

                    <form
                        class="flex flex-col gap-3"
                        novalidate
                        on:submit=move |ev: leptos::ev::SubmitEvent| {
                            ev.prevent_default();
                            save();
                        }
                    >
                        <div class="w-full">
                            <label class="mb-2 text-lg font-semibold label-text" for="edit-code">
                                "Code"
                            </label>
                            <input
                                id="edit-code"
                                node_ref=code_input
                                class=move || {
                                    let bad = code_error.get().is_some()
                                        || invalid.get() == Some("code");
                                    format!("font-mono {}", input_class(bad))
                                }
                                autocomplete="off"
                                aria-invalid=move || code_error.get().is_some().to_string()
                                aria-describedby="edit-code-error"
                                prop:value=code
                                on:input=move |ev| {
                                    code.set(event_target_value(&ev));
                                    invalid.set(None);
                                }
                                on:blur=move |_| touched.set(true)
                            />
                            <p id="edit-code-error" class="mt-2 text-sm text-error">
                                {move || code_error.get().unwrap_or_default()}
                            </p>
                        </div>

                        <div class="w-full">
                            <label class="mb-2 text-lg font-semibold label-text" for="edit-url">
                                "Destination URL"
                            </label>
                            <input
                                id="edit-url"
                                class=move || {
                                    input_class(
                                        url_error.get().is_some() || invalid.get() == Some("url"),
                                    )
                                }
                                placeholder="https://example.com"
                                autocomplete="off"
                                aria-invalid=move || url_error.get().is_some().to_string()
                                aria-describedby="edit-url-error"
                                prop:value=url
                                on:input=move |ev| {
                                    url.set(event_target_value(&ev));
                                    invalid.set(None);
                                }
                                on:blur=move |_| touched.set(true)
                            />
                            <p id="edit-url-error" class="mt-2 text-sm text-error">
                                {move || url_error.get().unwrap_or_default()}
                            </p>
                        </div>

                        <div class="w-full">
                            <label class="mb-2 text-lg font-semibold label-text" for="edit-expires">
                                "Expires"
                                <span class="ml-2 text-base font-normal opacity-60">
                                    "optional"
                                </span>
                            </label>
                            <DateTimePicker
                                id="edit-expires"
                                value=expiry.read_only()
                                set_value=expiry.write_only()
                                invalid=Signal::derive(move || {
                                    expiry_error.get().is_some()
                                        || invalid.get() == Some("expires_at")
                                })
                                disabled=saving
                                describedby="edit-expires-error"
                                open=popover
                                on_blur=Callback::new(move |()| touched.set(true))
                            />
                            <p id="edit-expires-error" class="mt-2 text-sm text-error">
                                {move || expiry_error.get().unwrap_or_default()}
                            </p>
                        </div>
                        <Show when=colliding>
                            {move || {
                                conflict
                                    .get()
                                    .map(|(existing, renaming)| {
                                        let owner = other_owner.get();
                                        let name = owner
                                            .as_ref()
                                            .and_then(|owner| owner.username.clone());
                                        view! {
                                            <p class="mt-6 mb-4 text-base-content/70">
                                                {conflict_message(
                                                    &existing.code,
                                                    name.as_deref(),
                                                    renaming,
                                                )}
                                            </p>
                                            <ReplacementDetails
                                                existing=existing
                                                url=url
                                                expires=expiry
                                                reset_hits=reset_hits
                                                claim=(!renaming && owner.is_some()).then_some(claim)
                                                other_owner=owner
                                            />
                                        }
                                    })
                            }}
                        </Show>

                        <div class="flex gap-2 justify-end mt-3">
                            <button class="btn btn-text" type="button" on:click=move |_| close()>
                                "Cancel"
                            </button>
                            <button
                                class=move || {
                                    let destructive = matches!(conflict.get(), Some((_, true)));
                                    format!(
                                        "btn {}",
                                        if destructive { "btn-error" } else { "btn-primary" },
                                    )
                                }
                                type="submit"
                                disabled=move || saving.get()
                            >
                                {move || if colliding() { "Replace" } else { "Save" }}
                            </button>
                        </div>
                    </form>
                </div>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rename destroys the other link, so it has to say so; everything else
    /// is a replacement in place.
    #[test]
    fn a_collision_says_whose_link_it_is_and_what_goes() {
        assert_eq!(
            conflict_message("x", Some("alice"), true),
            "/x belongs to alice. Moving this link onto it deletes theirs."
        );
        assert_eq!(
            conflict_message("x", Some("alice"), false),
            "/x belongs to alice."
        );
        assert!(conflict_message("x", None, true).contains("deletes it"));
        assert_eq!(conflict_message("x", None, false), "You already use /x.");
    }
}
