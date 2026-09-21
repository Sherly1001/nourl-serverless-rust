use std::time::Duration;

use leptos::portal::Portal;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

/// Widest the bubble is allowed to get, matching the `max-w-96` below. Only the
/// starting guess: what it is actually laid out at is measured once it is up.
const TOOLTIP_WIDTH_PX: f64 = 384.0;
/// Gap kept between the bubble and the window edge.
const VIEWPORT_MARGIN_PX: f64 = 8.0;
/// Distance between the bubble's arrow and the thing it points at.
const ANCHOR_GAP_PX: f64 = 8.0;
/// Pause before opening, so sweeping the pointer across a table does not
/// trail bubbles behind it.
const OPEN_DELAY: Duration = Duration::from_millis(400);

/// How much room above the anchor the bubble wants before it opens upwards.
/// Below this it opens downwards instead — the top rows of a scrolling table
/// have a sticky header over them, and a bubble there explains nothing.
const TOOLTIP_MIN_ROOM_PX: f64 = 120.0;

/// Anchored to the cell's left edge, pushed back to stay on screen, never
/// negative. `bubble_width` is the real width: clamping a short label as
/// `max-w-96` would drag it hundreds of pixels off target.
fn clamp_left(anchor_left: f64, bubble_width: f64, viewport_width: f64) -> f64 {
    let rightmost = viewport_width - bubble_width - VIEWPORT_MARGIN_PX;
    anchor_left.min(rightmost.max(VIEWPORT_MARGIN_PX)).max(0.0)
}

/// How close to a corner before the rounded border cuts into it.
const ARROW_SIZE_PX: f64 = 8.0;
const ARROW_INSET_PX: f64 = 8.0;

/// Tracks the anchor rather than sitting at a fixed inset, since [`clamp_left`]
/// slides the bubble — stopping at the bubble's own corners.
fn arrow_left(anchor_left: f64, bubble_left: f64, bubble_width: f64) -> f64 {
    // A little in, so the arrow lands over the anchor rather than beside.
    let target = anchor_left + ARROW_INSET_PX - bubble_left;
    let rightmost = bubble_width - ARROW_SIZE_PX - ARROW_INSET_PX;
    target.clamp(ARROW_INSET_PX, rightmost.max(ARROW_INSET_PX))
}

fn viewport_width() -> f64 {
    web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|value| value.as_f64())
        .unwrap_or(1024.0)
}

/// A bubble on hover, revealing a clipped value or labelling an icon-only
/// button. `fixed` to escape the cell's overflow, and portalled to `<body>` to
/// escape its stacking context — a pinned cell's `z-index` would bury it.
#[component]
pub fn Tooltip(
    /// The full text, shown only when it does not already fit.
    #[prop(into)]
    text: String,
    /// The default truncates; a button wrapper lays out inline instead.
    #[prop(default = "block truncate")]
    class: &'static str,
    /// False for a label that should always show, not only when clipped.
    #[prop(default = true)]
    only_when_clipped: bool,
    /// `ChildrenFn`: a row rebuilds its cells, so these are built more than once.
    children: ChildrenFn,
) -> impl IntoView {
    let (shown, set_shown) = signal(false);
    // Where the anchor was when the pointer arrived, in viewport coordinates.
    let (position, set_position) = signal((0.0_f64, 0.0_f64));
    // Whether it opened downwards, which moves the arrow to the top edge.
    let (flipped, set_flipped) = signal(false);
    // Guessed at the maximum until it is up and can be measured.
    let (bubble_width, set_bubble_width) = signal(TOOLTIP_WIDTH_PX);
    let bubble: NodeRef<leptos::html::Span> = NodeRef::new();
    Effect::new(move |_| {
        if let Some(node) = bubble.get() {
            set_bubble_width.set(f64::from(node.offset_width()));
        }
    });
    // Bumped on enter and leave, so a late timer knows it is stale.
    let (hover, set_hover) = signal(0u32);
    // `Portal` rebuilds its children, so the text is read more than once.
    let body = StoredValue::new(text.clone());

    // Anything that moves the anchor takes the bubble down: a wheel or a
    // dragged scrollbar moves it without moving the pointer, so `mouseleave`
    // never fires. Capture phase, since `scroll` does not bubble.
    type Listener = send_wrapper::SendWrapper<(web_sys::Document, Closure<dyn FnMut()>)>;
    let listener: StoredValue<Option<Listener>> = StoredValue::new(None);
    let unlisten = move || {
        listener.update_value(|slot| {
            if let Some(held) = slot.take() {
                let (document, hide) = held.take();
                for event in ["scroll", "resize"] {
                    let _ = document.remove_event_listener_with_callback_and_bool(
                        event,
                        hide.as_ref().unchecked_ref(),
                        true,
                    );
                }
            }
        });
    };
    Effect::new(move |_| {
        let up = shown.get();
        unlisten();
        if !up {
            return;
        }
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let hide = Closure::<dyn FnMut()>::new(move || {
            set_hover.try_update(|n| *n += 1);
            set_shown.try_set(false);
        });
        for event in ["scroll", "resize"] {
            let _ = document.add_event_listener_with_callback_and_bool(
                event,
                hide.as_ref().unchecked_ref(),
                true,
            );
        }
        listener.set_value(Some(send_wrapper::SendWrapper::new((document, hide))));
    });
    // A leak outlives its signals and panics on the next scroll anywhere.
    on_cleanup(unlisten);

    view! {
        <span
            class=class
            on:mouseenter=move |ev| {
                let Some(anchor) = ev
                    .current_target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else {
                    return;
                };
                if only_when_clipped && anchor.scroll_width() <= anchor.client_width() {
                    return;
                }
                let rect = anchor.get_bounding_client_rect();
                let above = rect.top() >= TOOLTIP_MIN_ROOM_PX;
                set_flipped.set(!above);
                set_position
                    .set(
                        if above {
                            (rect.left(), rect.top() - ANCHOR_GAP_PX)
                        } else {
                            (rect.left(), rect.bottom() + ANCHOR_GAP_PX)
                        },
                    );
                set_hover.update(|n| *n += 1);
                let mine = hover.get_untracked();
                set_timeout(
                    move || {
                        if hover.try_get_untracked() == Some(mine) {
                            set_shown.try_set(true);
                        }
                    },
                    OPEN_DELAY,
                );
            }
            on:mouseleave=move |_| {
                set_hover.update(|n| *n += 1);
                set_shown.set(false);
            }
        >
            {children()}
        </span>
        <Show when=move || shown.get()>
            <Portal>
                <span
                    node_ref=bubble
                    class=move || {
                        format!(
                            "fixed z-50 py-1 px-2 text-sm whitespace-normal break-all rounded border shadow-lg pointer-events-none bg-base-100 border-base-content/10 text-base-content max-w-96 motion-preset-fade motion-duration-150 {}",
                            if flipped.get() { "" } else { "-translate-y-full" },
                        )
                    }
                    role="tooltip"
                    style=move || {
                        let (left, top) = position.get();
                        let left = clamp_left(left, bubble_width.get(), viewport_width());
                        format!("left: {left}px; top: {top}px")
                    }
                >
                    {body.get_value()}
                    <span
                        class=move || {
                            format!(
                                "absolute border-r border-b rotate-45 size-2 bg-base-100 border-base-content/10 {}",
                                if flipped.get() { "-top-1 rotate-225" } else { "-bottom-1" },
                            )
                        }
                        style=move || {
                            let (anchor, _) = position.get();
                            let width = bubble_width.get();
                            let bubble = clamp_left(anchor, width, viewport_width());
                            format!("left: {}px", arrow_left(anchor, bubble, width))
                        }
                    ></span>
                </span>
            </Portal>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bubble_near_the_right_edge_is_pulled_back() {
        // Plenty of room: sits exactly under the cell.
        assert_eq!(clamp_left(100.0, 384.0, 1440.0), 100.0);
        // Too close to the edge: pulled left so the whole bubble is visible.
        assert_eq!(clamp_left(1300.0, 384.0, 1440.0), 1440.0 - 384.0 - 8.0);
    }

    /// A short label on a button at the right edge: pulling it back by the
    /// widest a bubble may get would leave it pointing at nothing.
    #[test]
    fn a_narrow_bubble_is_only_pulled_back_by_its_own_width() {
        assert_eq!(clamp_left(1300.0, 48.0, 1440.0), 1300.0);
        assert_eq!(clamp_left(1420.0, 48.0, 1440.0), 1440.0 - 48.0 - 8.0);
    }

    /// On a window narrower than the bubble the clamp would go negative, which
    /// would hide the start of the text off the left edge.
    #[test]
    fn a_narrow_window_still_starts_on_screen() {
        assert_eq!(clamp_left(10.0, 384.0, 320.0), 8.0);
        assert_eq!(clamp_left(0.0, 384.0, 320.0), 0.0);
    }

    /// The case from the admin table: a wide bubble on a control near the right
    /// edge. The bubble slides left to stay on screen, and the arrow has to
    /// follow it or it points at a different row's cell.
    #[test]
    fn the_arrow_follows_the_anchor_when_the_bubble_slides() {
        let (anchor, width, viewport) = (975.0, 402.0, 1225.0);
        let bubble = clamp_left(anchor, width, viewport);
        assert!(bubble < anchor, "this case only matters once it slides");
        // Still over the anchor, not stuck at the bubble's left corner.
        let arrow = arrow_left(anchor, bubble, width);
        assert_eq!(bubble + arrow, anchor + 8.0);
        assert!(arrow > 8.0, "not pinned to the corner: {arrow}");
    }

    #[test]
    fn the_arrow_never_leaves_the_bubble() {
        // Room to spare: sits at the near corner, where it always used to.
        assert_eq!(arrow_left(100.0, 100.0, 384.0), 8.0);
        // An anchor past the bubble's far edge — a very wide control — keeps
        // the arrow inside, clear of the rounded corner.
        assert_eq!(arrow_left(900.0, 100.0, 384.0), 384.0 - 8.0 - 8.0);
        // A bubble narrower than its own margins cannot satisfy both, and the
        // near corner wins rather than the value going negative.
        assert_eq!(arrow_left(900.0, 100.0, 12.0), 8.0);
    }
}
