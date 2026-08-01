use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

/// Closes a menu on a pointer press outside `root`, or on Escape.
///
/// The listeners sit on the document because a click elsewhere on the page
/// never reaches the menu's own subtree. Shared by every dropdown in the app —
/// FlyonUI's own dropdown needs its JavaScript runtime, which this build does
/// not ship.
pub fn dismiss_on_outside_click(root: NodeRef<leptos::html::Div>, set_open: WriteSignal<bool>) {
    let pointer = Closure::<dyn FnMut(web_sys::Event)>::new(move |ev: web_sys::Event| {
        let Some(container) = root.get_untracked() else {
            return;
        };
        let inside = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
            .is_some_and(|node| container.contains(Some(&node)));
        if !inside {
            set_open.set(false);
        }
    });
    let keydown =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" {
                set_open.set(false);
            }
        });
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        let _ = document
            .add_event_listener_with_callback("pointerdown", pointer.as_ref().unchecked_ref());
        let _ =
            document.add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref());
    }
    pointer.forget();
    keydown.forget();
}
