use leptos::html::Input;
use leptos::prelude::*;

use crate::datetime::{
    add_months, days_in_month, from_display, mask_display, month_grid, today_local,
};
use crate::dropdown::dismiss_on_outside_click;
use crate::ui::input_class;

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

/// How tall the popover gets. Only used to decide which side it opens on, so a
/// bound is enough.
const POPOVER_HEIGHT: f64 = 380.0;

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
    /// Runs when the field is left, which is when a half-typed date is worth
    /// complaining about.
    #[prop(optional)]
    on_blur: Option<Callback<()>>,
    /// Whether the popover is showing. Supply one to keep Escape from reaching
    /// past the popover to whatever encloses it.
    #[prop(optional)]
    open: Option<RwSignal<bool>>,
) -> impl IntoView {
    let open = open.unwrap_or_else(|| RwSignal::new(false));
    let (pane, set_pane) = signal(Pane::Days);
    // Which side of the field the popover hangs from, decided once on open.
    let (above, set_above) = signal(false);
    let root: NodeRef<leptos::html::Div> = NodeRef::new();
    let text_input: NodeRef<Input> = NodeRef::new();
    let hour_input: NodeRef<Input> = NodeRef::new();
    let minute_input: NodeRef<Input> = NodeRef::new();
    dismiss_on_outside_click(root, open.write_only());

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
        open.set(false);
        focus_field(text_input);
    };

    let write = move |y: i64, m: i64, d: i64, hour: i64, minute: i64| {
        set_value.set(format!("{y:04}/{m:02}/{d:02} {hour:02}:{minute:02}"));
    };

    // Keeps the time entered; a first pick starts at midnight.
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
        open.set(false);
    };

    let clearable = Memo::new(move |_| !value.get().is_empty() && !disabled.get());

    let button = "cursor-pointer btn btn-text btn-sm btn-square text-base-content/60 hover:text-base-content";

    let toggle = move |_| {
        if open.get_untracked() {
            open.set(false);
            return;
        }
        set_pane.set(Pane::Days);
        set_above.set(opens_above(root));
        open.set(true);
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
                    let room = if clearable.get() { "pe-20" } else { "pe-12" };
                    format!("{} w-full {room}", input_class(invalid.get()))
                }
                aria-invalid=move || invalid.get().to_string()
                aria-describedby=describedby
                disabled=move || disabled.get()
                node_ref=text_input
                prop:value=value
                on:focus=move |_| open.set(false)
                on:click=move |_| open.set(false)
                on:input=move |ev| {
                    let typed: String = event_target_value(&ev)
                        .chars()
                        .filter(|c| c.is_ascii_digit() || matches!(c, '/' | ':' | ' '))
                        .collect();
                    let deleting = typed.len() < value.get_untracked().len();
                    let masked = mask_display(&typed);
                    set_value
                        .set(
                            if deleting {
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
                    if ev.key() == "Enter" && open.get_untracked() {
                        ev.prevent_default();
                        open.set(false);
                    }
                }
            />

            <div class="flex absolute inset-y-0 right-2 items-center">
                <Show when=move || clearable.get()>
                    <button class=button type="button" aria-label="Clear expiry" on:click=clear>
                        <span class="icon-[tabler--x] size-5"></span>
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
                    <span class="icon-[tabler--calendar] size-5"></span>
                </button>
            </div>

            <Show when=move || open.get()>
                <div
                    class=move || {
                        let side = if above.get() { "bottom-full mb-2" } else { "top-full mt-2" };
                        format!(
                            "absolute right-0 z-50 p-3 w-80 border shadow-lg card bg-base-100 border-base-content/10 motion-preset-fade motion-duration-200 {side}",
                        )
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

    // Follows the value when something else moves it, not while typing.
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

/// Whether the popover has to hang above the field to stay in the window.
/// Below by default; it flips only when there is no room there and more room
/// the other way.
fn opens_above(root: NodeRef<leptos::html::Div>) -> bool {
    let Some(element) = root.get_untracked() else {
        return false;
    };
    let Some(window) = web_sys::window() else {
        return false;
    };
    let viewport = window
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let rect = element.get_bounding_client_rect();
    let below = viewport - rect.bottom();
    below < POPOVER_HEIGHT && rect.top() > below
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
