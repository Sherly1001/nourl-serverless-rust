use leptos::prelude::*;

use crate::dropdown::dismiss_on_outside_click;

/// A modal confirmation, replacing `window.confirm`.
///
/// The browser dialog cannot be styled, blocks the whole page, and on some
/// browsers is suppressed entirely after repeated use — which would silently
/// turn "delete" into a no-op.
///
/// Closing is handled here: the caller only says what to do on confirm.
#[component]
pub fn ConfirmDialog(
    open: RwSignal<bool>,
    #[prop(into)] title: Signal<String>,
    #[prop(into)] message: Signal<String>,
    #[prop(default = "Delete")] confirm_label: &'static str,
    /// Colour of the confirm button. Red by default, because most of what
    /// wants confirming is a deletion — but taking a flag away is not the same
    /// weight as destroying an account, and should not look like it.
    #[prop(into, default = Signal::derive(|| "btn-error".to_string()))]
    confirm_class: Signal<String>,
    /// Anything the caller needs answered before confirming — a choice of what
    /// to do, say. Drawn between the message and the buttons. A `ViewFn`
    /// because the dialog's body is rebuilt every time it opens.
    #[prop(optional, into)]
    extra: Option<ViewFn>,
    /// Holds the confirm button shut while `extra` is still missing something
    /// it asked for — a password, say. Cancel always stays live: a dialog you
    /// cannot answer must still be one you can leave.
    #[prop(optional, into)]
    confirm_disabled: Signal<bool>,
    on_confirm: Callback<()>,
) -> impl IntoView {
    let card: NodeRef<leptos::html::Div> = NodeRef::new();
    // Stored so the `Show` body, which is an `Fn`, can reach it more than once.
    let extra = StoredValue::new(extra);
    // Backdrop click and Escape both dismiss — the same helper the dropdowns
    // use, since "outside the card" is exactly the backdrop.
    dismiss_on_outside_click(card, open.write_only());

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
                        <button class="btn btn-text" on:click=move |_| open.set(false)>
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
