use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

/// Closes a menu on a pointer press outside `root`, or on Escape.
///
/// The listeners sit on the document because a click elsewhere on the page
/// never reaches the menu's own subtree. Shared by every dropdown in the app —
/// FlyonUI's own dropdown needs its JavaScript runtime, which this build does
/// not ship.
///
/// Both listeners are taken off the document again when the owner is dropped,
/// and every signal is read with its `try_` form. A leaked listener outlives
/// the menu it belongs to and panics on the next pointer press anywhere on the
/// page, because the node and the signal it reaches for have been disposed
/// along with their owner.
pub fn dismiss_on_outside_click(root: NodeRef<leptos::html::Div>, set_open: WriteSignal<bool>) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let pointer = Closure::<dyn FnMut(web_sys::Event)>::new(move |ev: web_sys::Event| {
        let Some(Some(container)) = root.try_get_untracked() else {
            return;
        };
        let inside = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
            .is_some_and(|node| container.contains(Some(&node)));
        if !inside {
            set_open.try_set(false);
        }
    });
    let keydown =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" {
                set_open.try_set(false);
            }
        });
    let _ =
        document.add_event_listener_with_callback("pointerdown", pointer.as_ref().unchecked_ref());
    let _ = document.add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref());
    // The cleanup owns the closures, which is what keeps them alive for exactly
    // as long as the listeners are registered. Wrapped because `on_cleanup`
    // wants `Send + Sync` for server rendering; there is one thread here.
    let handles = send_wrapper::SendWrapper::new((document, pointer, keydown));
    on_cleanup(move || {
        let (document, pointer, keydown) = handles.take();
        let _ = document
            .remove_event_listener_with_callback("pointerdown", pointer.as_ref().unchecked_ref());
        let _ = document
            .remove_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref());
    });
}
