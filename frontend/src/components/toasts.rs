use std::time::Duration;

use leptos::prelude::*;

use crate::toast::{Toast, use_toasts};

/// How often the countdown ticks. Small enough that the progress bar moves
/// smoothly, large enough not to churn.
const TICK_MS: u32 = 50;

/// Remaining lifetime as a percentage, for the progress bar's width.
fn percent_left(remaining: u32, total: u32) -> u32 {
    if total == 0 {
        return 0;
    }
    (remaining * 100) / total
}

/// One toast, owning its own countdown.
///
/// The timer lives here rather than in the queue so that hovering pauses only
/// the toast under the pointer, and so a dismissed toast takes its timer with
/// it.
#[component]
fn ToastCard(toast: Toast) -> impl IntoView {
    let toasts = use_toasts();
    let total = toast.kind.lifetime_ms();
    let (remaining, set_remaining) = signal(total);
    // Reading a message takes time; the countdown waits while the pointer is
    // over the card.
    let (paused, set_paused) = signal(false);
    let id = toast.id;

    let handle = set_interval_with_handle(
        move || {
            if paused.get_untracked() {
                return;
            }
            let next = remaining.get_untracked().saturating_sub(TICK_MS);
            set_remaining.set(next);
            if next == 0 {
                toasts.dismiss(id);
            }
        },
        Duration::from_millis(u64::from(TICK_MS)),
    )
    .ok();
    // Without this the interval keeps firing against a signal whose owner is
    // gone once the toast is dismissed.
    on_cleanup(move || {
        if let Some(handle) = handle {
            handle.clear();
        }
    });

    // `mt-0.5` centres it on the *first* line, not the whole message: 2px, the
    // gap between the 20px icon and a 24px line box. The 24px ✕ needs none.
    let icon = format!("{} size-5 shrink-0 mt-0.5", toast.kind.icon_class());
    let message = toast.message.clone();

    view! {
        // One element carries the alert colours, so the bar below is drawn on
        // the card rather than behind it — as a sibling it was painted over by
        // the alert's own background.
        <div
            class=format!(
                "flex overflow-hidden flex-col gap-0 p-0 w-80 max-w-full rounded-lg border-0 shadow-lg shrink-0 pointer-events-auto motion-preset-slide-left motion-duration-200 alert {}",
                toast.kind.alert_class(),
            )
            role="status"
            on:mouseenter=move |_| set_paused.set(true)
            on:mouseleave=move |_| set_paused.set(false)
        >
            // Across the top of the card, draining as the timer runs down.
            <div class="w-full h-1 shrink-0 bg-black/10">
                <div
                    class="h-full transition-all ease-linear bg-black/40 duration-50"
                    style=move || { format!("width: {}%", percent_left(remaining.get(), total)) }
                ></div>
            </div>

            <div class="flex gap-3 items-start p-4">
                <span class=icon.clone()></span>
                <span class="flex-1 break-words">{message.clone()}</span>
                // Deliberately not a `btn`: `.btn-text` sets its own neutral
                // colour, which ignores the alert's `--color-*-content` and
                // leaves the ✕ a different shade from the message beside it.
                // With no colour of its own it simply inherits.
                <button
                    class="inline-flex justify-center items-center rounded opacity-70 transition hover:opacity-100 shrink-0 size-6 hover:bg-black/10"
                    aria-label="Dismiss"
                    on:click=move |_| toasts.dismiss(id)
                >
                    <span class="icon-[tabler--x] size-4"></span>
                </button>
            </div>
        </div>
    }
}

/// The stack itself: fixed to the top right, newest at the bottom of the pile.
///
/// `pointer-events-none` on the container keeps the empty space beside the
/// toasts clickable; each card turns pointer events back on for itself.
#[component]
pub fn ToastStack() -> impl IntoView {
    let toasts = use_toasts();

    view! {
        // `top-20` clears the navbar; the height cap keeps a full stack from
        // running off the bottom of the window.
        // `overflow-x-clip` because the entry animation slides each card in from
        // the right, which would otherwise widen the scroll area for a moment.
        <div class="flex overflow-y-auto fixed right-4 top-20 z-50 flex-col gap-2 items-end pointer-events-none overflow-x-clip max-h-[calc(100vh-6rem)]">
            <For each=move || toasts.items() key=|toast| toast.id let:toast>
                <ToastCard toast=toast />
            </For>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_drains_from_full_to_empty() {
        assert_eq!(percent_left(3_000, 3_000), 100);
        assert_eq!(percent_left(1_500, 3_000), 50);
        assert_eq!(percent_left(0, 3_000), 0);
    }

    /// A zero lifetime would otherwise divide by zero.
    #[test]
    fn a_zero_lifetime_is_simply_empty() {
        assert_eq!(percent_left(0, 0), 0);
    }
}
