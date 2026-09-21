use leptos::prelude::*;

use crate::dropdown::dismiss_on_outside_click;

/// A modal confirmation. Not `window.confirm`, which cannot be styled and is
/// suppressed after repeated use — silently turning "delete" into a no-op.
/// Closing is handled here; the caller only says what confirming does.
#[component]
pub fn ConfirmDialog(
    open: RwSignal<bool>,
    #[prop(into)] title: Signal<String>,
    #[prop(into)] message: Signal<String>,
    #[prop(default = "Delete")] confirm_label: &'static str,
    /// Red by default, but taking a flag away should not look like deleting.
    #[prop(into, default = Signal::derive(|| "btn-error".to_string()))]
    confirm_class: Signal<String>,
    /// Drawn between message and buttons; a `ViewFn`, since it is rebuilt.
    #[prop(optional, into)]
    extra: Option<ViewFn>,
    /// Holds confirm shut while `extra` wants something. Cancel stays live.
    #[prop(optional, into)]
    confirm_disabled: Signal<bool>,
    on_confirm: Callback<()>,
) -> impl IntoView {
    let card: NodeRef<leptos::html::Div> = NodeRef::new();
    let cancel: NodeRef<leptos::html::Button> = NodeRef::new();
    // Stored so the `Show` body, which is an `Fn`, can reach it more than once.
    let extra = StoredValue::new(extra);
    // "Outside the card" is exactly the backdrop.
    dismiss_on_outside_click(card, open.write_only());

    // Cancel: most of this deletes, and it is the one button never disabled.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        request_animation_frame(move || {
            if let Some(button) = cancel.get_untracked() {
                let _ = button.focus();
            }
        });
    });

    view! {
        <Show when=move || open.get()>
            <div
                class="flex fixed inset-0 z-50 justify-center items-center p-4 bg-black/50 motion-preset-fade motion-duration-200"
                role="dialog"
                aria-modal="true"
            >
                <div
                    node_ref=card
                    class="p-6 w-full max-w-md rounded-lg border shadow-xl bg-base-100 border-base-content/10 motion-preset-slide-down motion-duration-200"
                >
                    <h3 class="mb-2 text-xl font-semibold">{move || title.get()}</h3>
                    <p class="mb-6 text-base-content/70">{move || message.get()}</p>
                    {move || extra.with_value(|slot| slot.as_ref().map(ViewFn::run))}
                    <div class="flex gap-2 justify-end">
                        <button
                            node_ref=cancel
                            class="btn btn-text"
                            on:click=move |_| open.set(false)
                        >
                            "Cancel"
                        </button>
                        <button
                            class=move || format!("btn {}", confirm_class.get())
                            disabled=move || confirm_disabled.get()
                            on:click=move |_| {
                                if confirm_disabled.get_untracked() {
                                    return;
                                }
                                open.set(false);
                                on_confirm.run(());
                            }
                        >
                            {confirm_label}
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}
