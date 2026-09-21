use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

/// Closes a menu on a pointer press outside `root`, or on Escape. On the
/// document, since a click elsewhere never reaches the menu's subtree. Removed
/// on drop and read with `try_`: a leak panics on the next press anywhere.
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
    // The cleanup owns the closures; wrapped because it wants `Send + Sync`.
    let handles = send_wrapper::SendWrapper::new((document, pointer, keydown));
    on_cleanup(move || {
        let (document, pointer, keydown) = handles.take();
        let _ = document
            .remove_event_listener_with_callback("pointerdown", pointer.as_ref().unchecked_ref());
        let _ = document
            .remove_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref());
    });
}
