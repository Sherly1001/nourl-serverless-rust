use std::time::Duration;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// Widest the bubble is allowed to get, matching the `max-w-96` below.
const TOOLTIP_WIDTH_PX: f64 = 384.0;
/// Gap kept between the bubble and the window edge.
const VIEWPORT_MARGIN_PX: f64 = 8.0;
/// Distance between the bubble's arrow and the thing it points at.
const ANCHOR_GAP_PX: f64 = 8.0;
/// Pause before opening, so sweeping the pointer across a table does not
/// trail bubbles behind it.
const OPEN_DELAY: Duration = Duration::from_millis(400);

/// Keeps the bubble inside the window: anchored to the cell's left edge, but
/// pushed back when that would run it off the right side, and never negative.
fn clamp_left(anchor_left: f64, viewport_width: f64) -> f64 {
    let rightmost = viewport_width - TOOLTIP_WIDTH_PX - VIEWPORT_MARGIN_PX;
    anchor_left.min(rightmost.max(VIEWPORT_MARGIN_PX)).max(0.0)
}

fn viewport_width() -> f64 {
    web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|value| value.as_f64())
        .unwrap_or(1024.0)
}

/// Shows `text` in a bubble on hover.
///
/// Two uses, one component: by default it truncates its children and reveals
/// the full value only when something was actually cut, which is what a table
/// cell wants. With `only_when_clipped=false` it always shows, which is what an
/// icon-only button wants — there the text is a label, not a repeat.
///
/// The bubble is `position: fixed` because the cell it lives in clips its
/// overflow — an absolutely positioned one would be cut off by the very
/// truncation it exists to explain. Fixed elements escape ancestor overflow, so
/// the coordinates are measured from the anchor at hover time.
#[component]
pub fn Tooltip(
    /// The full text, shown only when it does not already fit.
    #[prop(into)]
    text: String,
    /// Wrapper classes. The default truncates; a button wrapper wants to lay
    /// out inline instead.
    #[prop(default = "block truncate")]
    class: &'static str,
    /// When true (the default) the bubble appears only if the children were
    /// clipped. Set false for a label that should always show.
    #[prop(default = true)]
    only_when_clipped: bool,
    /// `ChildrenFn`, not `Children`: a table row rebuilds its cells whenever
    /// the row re-renders, so the children have to be constructible more than
    /// once.
    children: ChildrenFn,
) -> impl IntoView {
    let (shown, set_shown) = signal(false);
    let (position, set_position) = signal((0.0_f64, 0.0_f64));
    // Bumped on every enter and leave, so a timer that fires after the pointer
    // has moved on knows it is stale.
    let (hover, set_hover) = signal(0u32);
    let body = text.clone();

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
                set_position
                    .set((clamp_left(rect.left(), viewport_width()), rect.top() - ANCHOR_GAP_PX));
                set_hover.update(|n| *n += 1);
                let mine = hover.get_untracked();
                set_timeout(
                    move || {
                        if hover.get_untracked() == mine {
                            set_shown.set(true);
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
            <Show when=move || shown.get()>
                <span
                    class="fixed z-50 py-1 px-2 text-sm whitespace-normal break-all rounded border shadow-lg -translate-y-full pointer-events-none bg-base-100 border-base-content/10 text-base-content max-w-96 motion-preset-fade motion-duration-150"
                    role="tooltip"
                    style=move || {
                        let (left, top) = position.get();
                        format!("left: {left}px; top: {top}px")
                    }
                >
                    {body.clone()}
                    // A rotated square peeking out of the bottom edge, near the
                    // left so it points at the start of what it describes.
                    <span class="absolute -bottom-1 left-4 border-r border-b rotate-45 size-2 bg-base-100 border-base-content/10"></span>
                </span>
            </Show>
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bubble_near_the_right_edge_is_pulled_back() {
        // Plenty of room: sits exactly under the cell.
        assert_eq!(clamp_left(100.0, 1440.0), 100.0);
        // Too close to the edge: pulled left so the whole bubble is visible.
        assert_eq!(clamp_left(1300.0, 1440.0), 1440.0 - 384.0 - 8.0);
    }

    /// On a window narrower than the bubble the clamp would go negative, which
    /// would hide the start of the text off the left edge.
    #[test]
    fn a_narrow_window_still_starts_on_screen() {
        assert_eq!(clamp_left(10.0, 320.0), 8.0);
        assert_eq!(clamp_left(0.0, 320.0), 0.0);
    }
}
