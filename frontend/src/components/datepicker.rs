use leptos::html::Input;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

use crate::datetime::{
    add_months, days_in_month, from_display, mask_display, month_grid, today_local,
};
use crate::dropdown::dismiss_on_outside_click;
use crate::ui::{input_class, row_input_class};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// How tall the popover gets, used only until it has been rendered once and
/// can be measured. `w-80` is where the width comes from.
const POPOVER_HEIGHT: f64 = 380.0;
const POPOVER_WIDTH: f64 = 320.0;

/// Between the popover and the field, and between the popover and the edge of
/// the window.
const GAP: f64 = 8.0;

/// A year page is twelve, so that pane is the same grid as the months one.
const YEAR_PAGE: i64 = 12;

/// The header is the way up: the days pane names its month, the months pane
/// names its year, and each opens the pane above it.
#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Days,
    Months,
    Years,
}

/// A date and time field. FlyonUI's is flatpickr and `datetime-local` takes
/// its format from the locale, so neither fits. The signal holds what the
/// field shows, so text that parses to nothing survives to be corrected.
#[component]
pub fn DateTimePicker(
    #[prop(into)] id: String,
    value: ReadSignal<String>,
    set_value: WriteSignal<String>,
    #[prop(into)] invalid: Signal<bool>,
    #[prop(into)] disabled: Signal<bool>,
    #[prop(optional, into)] describedby: Option<&'static str>,
    /// Sized for a table row rather than for a form.
    #[prop(optional)]
    small: bool,
    /// Runs when the field is left, which is when a half-typed date is worth
    /// complaining about.
    #[prop(optional)]
    on_blur: Option<Callback<()>>,
    /// Keys the caller wants for itself, forwarded only while the popover is
    /// closed — the row editor commits on Enter and abandons on Escape, and
    /// both mean something to the popover first.
    #[prop(optional)]
    on_keydown: Option<Callback<leptos::ev::KeyboardEvent>>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let (pane, set_pane) = signal(Pane::Days);
    // Where the popover sits, in viewport coordinates. See `popover_spot`.
    let (spot, set_spot) = signal((0.0, 0.0));
    let root: NodeRef<leptos::html::Div> = NodeRef::new();
    let popover: NodeRef<leptos::html::Div> = NodeRef::new();
    let text_input: NodeRef<Input> = NodeRef::new();
    let hour_input: NodeRef<Input> = NodeRef::new();
    let minute_input: NodeRef<Input> = NodeRef::new();
    dismiss_on_outside_click(root, set_open);

    let place = move || {
        let Some(Some(field)) = root.try_get_untracked() else {
            return;
        };
        let height = popover
            .try_get_untracked()
            .flatten()
            .map(|p| p.get_bounding_client_rect().height())
            .filter(|h| *h > 0.0)
            .unwrap_or(POPOVER_HEIGHT);
        let _ = set_spot.try_set(popover_spot(&field, height));
    };
    follow_scroll(place);
    // Runs again if the node is replaced; the guard keeps it to one observer.
    Effect::new(move |watched: Option<bool>| {
        if watched == Some(true) {
            return true;
        }
        let Some(field) = root.get() else {
            return false;
        };
        close_when_out_of_view(&field, set_open);
        true
    });
    // After the popover exists, so the second pass places it by its real
    // height rather than by the guess.
    Effect::new(move |_| {
        if open.get() {
            place();
            request_animation_frame(place);
        }
    });

    // The month on screen: whatever is selected, else the month we are in.
    let (view_month, set_view_month) = signal(None::<(i64, i64)>);
    let showing = Memo::new(move |_| {
        view_month.get().unwrap_or_else(|| {
            from_display(&value.get())
                .and_then(|c| parse_parts(&c))
                .map(|(y, m, ..)| (y, m))
                .unwrap_or_else(|| {
                    let (y, m, _) = today_local();
                    (y, m)
                })
        })
    });
    let selected = Memo::new(move |_| from_display(&value.get()).and_then(|c| parse_parts(&c)));

    // The arrows mean whatever the visible pane counts in.
    let step = move |delta: i64| {
        let (y, m) = showing.get_untracked();
        set_view_month.set(Some(match pane.get_untracked() {
            Pane::Days => add_months(y, m, delta),
            Pane::Months => (y + delta, m),
            Pane::Years => (y + delta * YEAR_PAGE, m),
        }));
    };

    let focus_field = move |target: NodeRef<Input>| {
        request_animation_frame(move || {
            if let Some(input) = target.get_untracked() {
                let _ = input.focus();
                input.select();
            }
        });
    };

    // Leaving the popover puts the caller back where they started.
    let finish = move || {
        set_open.set(false);
        focus_field(text_input);
    };

    let write = move |y: i64, m: i64, d: i64, hour: i64, minute: i64| {
        set_value.set(format!("{y:04}/{m:02}/{d:02} {hour:02}:{minute:02}"));
    };

    // Picking a day keeps the time already entered; a first pick starts at
    // midnight, which is the earliest the chosen day can mean.
    let pick_day = move |(y, m, d): (i64, i64, i64)| {
        let (hour, minute) = selected
            .get_untracked()
            .map_or((0, 0), |(.., h, mi)| (h, mi));
        write(y, m, d, hour, minute);
        set_view_month.set(Some((y, m)));
        // The date is settled, so the time is what is left to say.
        focus_field(hour_input);
    };

    let set_clock = move |hour: i64, minute: i64| {
        let (y, m, d, ..) = selected.get_untracked().unwrap_or_else(|| {
            let (y, m, d) = today_local();
            (y, m, d, 0, 0)
        });
        write(y, m, d, hour, minute);
    };

    let clear = move |_| {
        set_value.set(String::new());
        set_view_month.set(None);
        set_open.set(false);
    };

    let clearable = Memo::new(move |_| !value.get().is_empty() && !disabled.get());

    // A row is tight enough that the buttons have to give the text its space
    // back; a form has room for the larger targets.
    let button = if small {
        "cursor-pointer btn btn-text btn-xs btn-square text-base-content/60 hover:text-base-content"
    } else {
        "cursor-pointer btn btn-text btn-sm btn-square text-base-content/60 hover:text-base-content"
    };

    let toggle = move |_| {
        if open.get_untracked() {
            set_open.set(false);
            return;
        }
        set_pane.set(Pane::Days);
        set_open.set(true);
    };

    view! {
        <div class="relative" node_ref=root>
            <input
                id=id
                type="text"
                inputmode="numeric"
                autocomplete="off"
                placeholder="yyyy/mm/dd hh:mm"
                class=move || {
                    let base = if small {
                        row_input_class(invalid.get())
                    } else {
                        input_class(invalid.get())
                    };
                    let room = match (small, clearable.get()) {
                        (true, true) => "pe-16",
                        (true, false) => "pe-9",
                        (false, true) => "pe-20",
                        (false, false) => "pe-12",
                    };
                    format!("{base} w-full {room}")
                }
                aria-invalid=move || invalid.get().to_string()
                aria-describedby=describedby
                disabled=move || disabled.get()
                node_ref=text_input
                prop:value=value
                on:focus=move |_| set_open.set(false)
                on:click=move |_| set_open.set(false)
                on:input=move |ev| {
                    let typed = event_target_value(&ev);
                    let deleting = typed.len() < value.get_untracked().len();
                    let masked = mask_display(&typed);
                    set_value
                        .set(
                            if masked.is_empty() && !typed.is_empty() {
                                typed
                            } else if deleting {
                                masked.trim_end_matches(|c: char| !c.is_ascii_digit()).to_string()
                            } else {
                                masked
                            },
                        );
                }
                on:blur=move |_| {
                    if let Some(callback) = on_blur {
                        callback.run(());
                    }
                }
                on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                    if !open.get_untracked() && let Some(callback) = on_keydown {
                        callback.run(ev);
                    }
                }
            />

            <div class=if small {
                "flex absolute inset-y-0 right-1 items-center"
            } else {
                "flex absolute inset-y-0 right-2 items-center"
            }>
                <Show when=move || clearable.get()>
                    <button class=button type="button" aria-label="Clear expiry" on:click=clear>
                        <span class=if small {
                            "icon-[tabler--x] size-4"
                        } else {
                            "icon-[tabler--x] size-5"
                        }></span>
                    </button>
                </Show>
                <button
                    class=button
                    type="button"
                    aria-label="Open calendar"
                    aria-haspopup="dialog"
                    aria-expanded=move || open.get().to_string()
                    disabled=move || disabled.get()
                    on:click=toggle
                >
                    <span class=if small {
                        "icon-[tabler--calendar] size-4"
                    } else {
                        "icon-[tabler--calendar] size-5"
                    }></span>
                </button>
            </div>

            <Show when=move || open.get()>
                <div
                    node_ref=popover
                    class="fixed z-50 p-3 w-80 border shadow-lg card bg-base-100 border-base-content/10 motion-preset-fade motion-duration-200"
                    style=move || {
                        let (left, top) = spot.get();
                        format!("left:{left}px;top:{top}px")
                    }
                    role="dialog"
                    aria-label="Choose a date and time"
                >
                    <div class="flex justify-between items-center mb-2">
                        <button
                            class="cursor-pointer btn btn-text btn-sm btn-square"
                            type="button"
                            aria-label=move || match pane.get() {
                                Pane::Days => "Previous month",
                                Pane::Months => "Previous year",
                                Pane::Years => "Earlier years",
                            }
                            on:click=move |_| step(-1)
                        >
                            <span class="icon-[tabler--chevron-left] size-5"></span>
                        </button>
                        <button
                            class="font-semibold cursor-pointer btn btn-text btn-sm"
                            type="button"
                            disabled=move || pane.get() == Pane::Years
                            on:click=move |_| {
                                set_pane
                                    .update(|p| {
                                        *p = match *p {
                                            Pane::Days => Pane::Months,
                                            _ => Pane::Years,
                                        };
                                    })
                            }
                        >
                            {move || {
                                let (y, m) = showing.get();
                                match pane.get() {
                                    Pane::Days => format!("{} {y}", MONTHS[(m - 1) as usize]),
                                    Pane::Months => y.to_string(),
                                    Pane::Years => {
                                        let first = year_page_start(y);
                                        format!("{first} – {}", first + YEAR_PAGE - 1)
                                    }
                                }
                            }}
                        </button>
                        <button
                            class="cursor-pointer btn btn-text btn-sm btn-square"
                            type="button"
                            aria-label=move || match pane.get() {
                                Pane::Days => "Next month",
                                Pane::Months => "Next year",
                                Pane::Years => "Later years",
                            }
                            on:click=move |_| step(1)
                        >
                            <span class="icon-[tabler--chevron-right] size-5"></span>
                        </button>
                    </div>

                    <div class="min-h-56">
                        <Show when=move || pane.get() == Pane::Days>
                            <div class="grid grid-cols-7 mb-1 text-xs text-center opacity-60">
                                {["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"]
                                    .into_iter()
                                    .map(|d| view! { <span>{d}</span> })
                                    .collect_view()}
                            </div>
                            <div class="grid grid-cols-7 gap-0.5">
                                {move || {
                                    let (year, month) = showing.get();
                                    let chosen = selected.get().map(|(y, m, d, ..)| (y, m, d));
                                    let today = today_local();
                                    month_grid(year, month)
                                        .into_iter()
                                        .map(|cell| {
                                            let (y, m, d) = cell;
                                            let mut class = String::from(
                                                "btn btn-sm btn-square cursor-pointer font-normal",
                                            );
                                            if Some(cell) == chosen {
                                                class.push_str(" btn-primary");
                                            } else {
                                                class.push_str(" btn-text");
                                                if cell == today {
                                                    class.push_str(" ring-1 ring-primary/60");
                                                }
                                                if m != month {
                                                    class.push_str(" opacity-40");
                                                }
                                            }
                                            view! {
                                                <button
                                                    class=class
                                                    type="button"
                                                    aria-label=format!("{y:04}-{m:02}-{d:02}")
                                                    on:click=move |_| pick_day(cell)
                                                >
                                                    {d}
                                                </button>
                                            }
                                        })
                                        .collect_view()
                                }}
                            </div>
                        </Show>

                        <Show when=move || pane.get() == Pane::Months>
                            <div class="grid grid-cols-3 gap-1">
                                {move || {
                                    let (year, month) = showing.get();
                                    (1..=12)
                                        .map(|m| {
                                            view! {
                                                <button
                                                    class=if m == month {
                                                        "cursor-pointer btn btn-sm btn-primary"
                                                    } else {
                                                        "cursor-pointer btn btn-sm btn-text"
                                                    }
                                                    type="button"
                                                    on:click=move |_| {
                                                        set_view_month.set(Some((year, m)));
                                                        set_pane.set(Pane::Days);
                                                    }
                                                >
                                                    {&MONTHS[(m - 1) as usize][..3]}
                                                </button>
                                            }
                                        })
                                        .collect_view()
                                }}
                            </div>
                        </Show>

                        <Show when=move || pane.get() == Pane::Years>
                            <div class="grid grid-cols-3 gap-1">
                                {move || {
                                    let (year, month) = showing.get();
                                    let first = year_page_start(year);
                                    (first..first + YEAR_PAGE)
                                        .map(|y| {
                                            view! {
                                                <button
                                                    class=if y == year {
                                                        "cursor-pointer btn btn-sm btn-primary"
                                                    } else {
                                                        "cursor-pointer btn btn-sm btn-text"
                                                    }
                                                    type="button"
                                                    on:click=move |_| {
                                                        set_view_month.set(Some((y, month)));
                                                        set_pane.set(Pane::Months);
                                                    }
                                                >
                                                    {y}
                                                </button>
                                            }
                                        })
                                        .collect_view()
                                }}
                            </div>
                        </Show>
                    </div>

                    <div class="flex gap-2 items-center pt-3 mt-3 border-t border-base-content/10">
                        <button
                            class="cursor-pointer btn btn-sm btn-text"
                            type="button"
                            aria-label="Select today"
                            on:click=move |_| {
                                set_pane.set(Pane::Days);
                                pick_day(today_local());
                            }
                        >
                            "Today"
                        </button>
                        <ClockField
                            label="Hour"
                            max=23
                            input_ref=hour_input
                            next=Some(minute_input)
                            part=Signal::derive(move || selected.get().map(|(.., h, _)| h))
                            on_done=Callback::new(move |_| finish())
                            on_set=Callback::new(move |hour: i64| {
                                let minute = selected.get_untracked().map_or(0, |(.., m)| m);
                                set_clock(hour, minute);
                            })
                        />
                        <span class="font-semibold">":"</span>
                        <ClockField
                            label="Minute"
                            max=59
                            input_ref=minute_input
                            next=None
                            part=Signal::derive(move || selected.get().map(|(.., m)| m))
                            on_done=Callback::new(move |_| finish())
                            on_set=Callback::new(move |minute: i64| {
                                let hour = selected.get_untracked().map_or(0, |(.., h, _)| h);
                                set_clock(hour, minute);
                            })
                        />
                        <button
                            class="ml-auto cursor-pointer btn btn-sm btn-primary"
                            type="button"
                            on:click=move |_| finish()
                        >
                            "Done"
                        </button>
                    </div>
                </div>
            </Show>
        </div>
    }
}

/// One half of the clock. It keeps its own text rather than rendering `part`:
/// reformatting every keystroke would rewrite `1` to `01` under the cursor,
/// and `19` could never be typed.
#[component]
fn ClockField(
    label: &'static str,
    max: i64,
    input_ref: NodeRef<Input>,
    /// Focused once this field can hold no more digits.
    next: Option<NodeRef<Input>>,
    part: Signal<Option<i64>>,
    on_set: Callback<i64>,
    /// Enter finishes the picker rather than the form the picker sits in.
    on_done: Callback<()>,
) -> impl IntoView {
    let (text, set_text) = signal(String::new());

    // Follows the value when something else moves it — a day picked for the
    // first time, or the field cleared — and leaves half-typed text alone.
    Effect::new(move |_| {
        let shown = part.get().map(|n| format!("{n:02}")).unwrap_or_default();
        if text.get_untracked().parse::<i64>().ok() != part.get() {
            set_text.set(shown);
        }
    });

    view! {
        <input
            class="w-14 text-center input input-sm"
            type="text"
            inputmode="numeric"
            maxlength="2"
            aria-label=label
            node_ref=input_ref
            prop:value=text
            on:focus=move |_| {
                if let Some(input) = input_ref.get_untracked() {
                    input.select();
                }
            }
            on:input=move |ev| {
                let digits: String = event_target_value(&ev)
                    .chars()
                    .filter(|c| c.is_ascii_digit())
                    .take(2)
                    .collect();
                set_text.set(digits.clone());
                let Ok(n) = digits.parse::<i64>() else { return };
                if n > max {
                    return;
                }
                on_set.run(n);
                if (digits.len() == 2 || n * 10 > max)
                    && let Some(next) = next.and_then(|r| r.get_untracked())
                {
                    let _ = next.focus();
                    next.select();
                }
            }
            on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                if ev.key() == "Enter" {
                    ev.prevent_default();
                    on_done.run(());
                }
            }
            on:blur=move |_| {
                set_text.set(part.get_untracked().map(|n| format!("{n:02}")).unwrap_or_default());
            }
        />
    }
}

/// Where the popover goes, in viewport coordinates: `fixed`, or the scrolling
/// box around the field would clip it — hence [`follow_scroll`]. Below unless
/// there is no room, right-aligned, pulled back inside the window.
fn popover_spot(field: &web_sys::Element, height: f64) -> (f64, f64) {
    let Some(window) = web_sys::window() else {
        return (0.0, 0.0);
    };
    let axis = |value: Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>| {
        value.ok().and_then(|v| v.as_f64()).unwrap_or(0.0)
    };
    let (width, viewport) = (axis(window.inner_width()), axis(window.inner_height()));
    let rect = field.get_bounding_client_rect();

    let below = viewport - rect.bottom();
    let top = if below < height && rect.top() > below {
        rect.top() - height - GAP
    } else {
        rect.bottom() + GAP
    };
    let left = (rect.right() - POPOVER_WIDTH).min(width - POPOVER_WIDTH - GAP);
    (left.max(GAP), top.max(GAP))
}

/// Closes the popover once the field scrolls out of sight — being `fixed`, it
/// would otherwise hover over a row nobody can see. An observer, because the
/// field is clipped by its ancestors rather than by the window.
fn close_when_out_of_view(field: &web_sys::Element, set_open: WriteSignal<bool>) {
    let seen = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
        let gone = entries.iter().any(|entry| {
            entry
                .dyn_into::<web_sys::IntersectionObserverEntry>()
                .is_ok_and(|entry| !entry.is_intersecting())
        });
        if gone {
            set_open.try_set(false);
        }
    });
    let Ok(observer) = web_sys::IntersectionObserver::new(seen.as_ref().unchecked_ref()) else {
        return;
    };
    observer.observe(field);

    let handles = send_wrapper::SendWrapper::new((observer, seen));
    on_cleanup(move || {
        let (observer, _seen) = handles.take();
        observer.disconnect();
    });
}

/// Re-runs `place` on any scroll or resize. Capture phase, since a scroll
/// inside a div does not bubble. The cleanup owns the closures, as in
/// [`crate::dropdown::dismiss_on_outside_click`].
fn follow_scroll(place: impl Fn() + Clone + 'static) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let scrolled = Closure::<dyn FnMut()>::new({
        let place = place.clone();
        move || place()
    });
    let resized = Closure::<dyn FnMut()>::new(place);
    let _ = window.add_event_listener_with_callback_and_bool(
        "scroll",
        scrolled.as_ref().unchecked_ref(),
        true,
    );
    let _ = window.add_event_listener_with_callback("resize", resized.as_ref().unchecked_ref());

    let handles = send_wrapper::SendWrapper::new((window, scrolled, resized));
    on_cleanup(move || {
        let (window, scrolled, resized) = handles.take();
        let _ = window.remove_event_listener_with_callback_and_bool(
            "scroll",
            scrolled.as_ref().unchecked_ref(),
            true,
        );
        let _ =
            window.remove_event_listener_with_callback("resize", resized.as_ref().unchecked_ref());
    });
}

/// The first year of the twelve-year page `year` falls in.
fn year_page_start(year: i64) -> i64 {
    year.div_euclid(YEAR_PAGE) * YEAR_PAGE
}

/// `2026-08-20T01:54` split out for the grid and the clock.
fn parse_parts(canonical: &str) -> Option<(i64, i64, i64, i64, i64)> {
    let (date, time) = canonical.split_once('T')?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let mut t = time.split(':');
    let hour: i64 = t.next()?.parse().ok()?;
    let minute: i64 = t.next()?.parse().ok()?;
    if day > days_in_month(year, month) {
        return None;
    }
    Some((year, month, day, hour, minute))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parts_come_back_out_of_a_canonical_stamp() {
        assert_eq!(parse_parts("2026-08-20T01:54"), Some((2026, 8, 20, 1, 54)));
        assert_eq!(parse_parts("2026-02-30T01:54"), None);
        assert_eq!(parse_parts("2026-08-20"), None);
    }

    #[test]
    fn a_year_page_holds_the_year_it_is_asked_about() {
        assert_eq!(year_page_start(2026), 2016);
        assert_eq!(year_page_start(2016), 2016);
        assert_eq!(year_page_start(2015), 2004);
        for year in [1999, 2024, 2100] {
            let first = year_page_start(year);
            assert!((first..first + YEAR_PAGE).contains(&year));
        }
    }
}
