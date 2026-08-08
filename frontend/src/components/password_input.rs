//! A password box with a reveal toggle, shared by every page that asks for one.
//!
//! The toggle exists because a password typed blind is a password typed wrong,
//! and the usual answer — a second "confirm" box — asks for the same mistake
//! twice.

use leptos::prelude::*;

use crate::ui::input_class;

/// What the input's `type` should be. A revealed field is a plain text box:
/// there is no `type` that shows its own value.
fn field_type(shown: bool) -> &'static str {
    if shown { "text" } else { "password" }
}

/// The toggle's accessible name, which has to say what clicking does rather
/// than what the field is currently doing.
fn toggle_label(shown: bool) -> &'static str {
    if shown {
        "Hide password"
    } else {
        "Show password"
    }
}

fn toggle_icon(shown: bool) -> &'static str {
    if shown {
        "icon-[tabler--eye-off] size-5"
    } else {
        "icon-[tabler--eye] size-5"
    }
}

#[component]
pub fn PasswordInput(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_input: Callback<String>,
    /// `current-password` where the browser should offer a saved one,
    /// `new-password` where it should offer to generate one instead. Reactive
    /// because the login page is both, depending on which mode it is in.
    #[prop(into, default = Signal::derive(|| "new-password".to_string()))]
    autocomplete: Signal<String>,
    #[prop(optional, into)] invalid: Signal<bool>,
    #[prop(optional)] id: Option<&'static str>,
    #[prop(optional)] describedby: Option<&'static str>,
    #[prop(optional)] node_ref: Option<NodeRef<leptos::html::Input>>,
    #[prop(optional)] on_blur: Option<Callback<()>>,
) -> impl IntoView {
    let shown = RwSignal::new(false);

    view! {
        <div class="relative">
            <input
                id=id
                node_ref=node_ref.unwrap_or_default()
                // `pe-12` keeps the text clear of the button sitting on top of
                // it, which is inside the box rather than beside it so the
                // field still reads as one control.
                class=move || format!("{} pe-12", input_class(invalid.get()))
                type=move || field_type(shown.get())
                autocomplete=move || autocomplete.get()
                aria-invalid=move || invalid.get().to_string()
                aria-describedby=describedby
                prop:value=move || value.get()
                on:input=move |ev| on_input.run(event_target_value(&ev))
                on:blur=move |_| {
                    if let Some(blur) = on_blur {
                        blur.run(());
                    }
                }
            />
            // `type=button`: inside a form, a bare button submits it, so
            // revealing the password would send it.
            <button
                type="button"
                class="flex absolute inset-y-0 right-0 items-center px-4 rounded-e-md text-base-content/60 hover:text-base-content"
                aria-label=move || toggle_label(shown.get())
                aria-pressed=move || shown.get().to_string()
                tabindex="-1"
                on:click=move |_| shown.update(|s| *s = !*s)
            >
                <span class=move || toggle_icon(shown.get())></span>
            </button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revealing_swaps_the_input_type() {
        assert_eq!(field_type(false), "password");
        assert_eq!(field_type(true), "text");
    }

    /// The label names the action, not the state: a button reading "Show
    /// password" while the password is already showing is a lie to anyone
    /// reading it aloud.
    #[test]
    fn the_toggle_is_labelled_by_what_it_does() {
        assert_eq!(toggle_label(false), "Show password");
        assert_eq!(toggle_label(true), "Hide password");
        assert_ne!(toggle_icon(false), toggle_icon(true));
    }
}
