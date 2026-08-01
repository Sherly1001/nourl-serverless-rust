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
    on_confirm: Callback<()>,
) -> impl IntoView {
    let card: NodeRef<leptos::html::Div> = NodeRef::new();
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
                    <div class="flex gap-2 justify-end">
                        <button class="btn btn-text" on:click=move |_| open.set(false)>
                            "Cancel"
                        </button>
                        <button
                            class="btn btn-error"
                            on:click=move |_| {
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
