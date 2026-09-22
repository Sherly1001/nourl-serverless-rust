use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::components::datepicker::DateTimePicker;
use crate::datetime::{local_offset_minutes, now_local};
use crate::filters::{DateRange, ExpiryChoice, Filters, OwnerChoice};
use crate::list::SEARCH_DEBOUNCE;
use crate::ui::input_class;

/// The native spinners square off the rounded border and ignore the theme.
const SPINNERLESS: &str = "[appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none";

#[component]
fn TextItem(
    label: &'static str,
    placeholder: &'static str,
    values: Signal<Vec<String>>,
    set_values: Callback<Vec<String>>,
    #[prop(optional)] input_ref: Option<NodeRef<leptos::html::Input>>,
) -> impl IntoView {
    let draft = RwSignal::new(String::new());
    let add = move || {
        let typed = draft.get_untracked().trim().to_string();
        if typed.is_empty() {
            return;
        }
        let mut next = values.get_untracked();
        if !next.contains(&typed) {
            next.push(typed);
            set_values.run(next);
        }
        draft.set(String::new());
    };

    view! {
        <div class="w-full">
            <label class="mb-2 font-semibold label-text">{label}</label>
            <input
                node_ref=input_ref.unwrap_or_default()
                class=input_class(false)
                placeholder=placeholder
                autocomplete="off"
                prop:value=draft
                on:input=move |ev| draft.set(event_target_value(&ev))
                on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                    if ev.key() == "Enter" && !draft.get_untracked().trim().is_empty() {
                        ev.prevent_default();
                        add();
                    }
                }
                on:blur=move |_| add()
            />
            <Chips values=values set_values=set_values />
        </div>
    }
}

#[component]
fn Chips(values: Signal<Vec<String>>, set_values: Callback<Vec<String>>) -> impl IntoView {
    view! {
        <Show when=move || !values.get().is_empty()>
            <div class="flex flex-wrap gap-1 mt-2">
                {move || {
                    values
                        .get()
                        .into_iter()
                        .map(|value| {
                            let mine = value.clone();
                            view! {
                                <button
                                    type="button"
                                    class="gap-1 badge badge-soft badge-primary"
                                    on:click=move |_| {
                                        let next: Vec<String> = values
                                            .get_untracked()
                                            .into_iter()
                                            .filter(|held| held != &mine)
                                            .collect();
                                        set_values.run(next);
                                    }
                                >
                                    {value.clone()}
                                    <span class="icon-[tabler--x] size-3"></span>
                                </button>
                            }
                        })
                        .collect_view()
                }}
            </div>
        </Show>
    }
}

#[component]
fn DateRangeItem(
    label: &'static str,
    range: Signal<DateRange>,
    set_range: Callback<DateRange>,
    /// One per picker: a shared signal opens every calendar at once.
    popovers: (RwSignal<bool>, RwSignal<bool>),
    /// What is wrong with the range, which paints both fields and says so.
    #[prop(into)]
    problem: Signal<Option<&'static str>>,
) -> impl IntoView {
    let (from, set_from) = signal(range.get_untracked().from);
    let (to, set_to) = signal(range.get_untracked().to);
    Effect::new(move |_| {
        let held = range.get();
        if from.get_untracked() != held.from {
            set_from.set(held.from.clone());
        }
        if to.get_untracked() != held.to {
            set_to.set(held.to);
        }
    });
    Effect::new(move |_| {
        let edited = DateRange {
            from: from.get(),
            to: to.get(),
        };
        if range.get_untracked() != edited {
            set_range.run(edited);
        }
    });

    view! {
        <div class="w-full">
            <label class="mb-2 font-semibold label-text">{label}</label>
            <div class="flex gap-2 items-center">
                <div class="flex-1 min-w-80">
                    <DateTimePicker
                        id=format!("filter-{}-from", label.to_lowercase().replace(' ', "-"))
                        value=from
                        set_value=set_from
                        invalid=Signal::derive(move || problem.get().is_some())
                        disabled=Signal::derive(|| false)
                        open=popovers.0
                    />
                </div>
                <span class="opacity-60">"to"</span>
                <div class="flex-1 min-w-80">
                    <DateTimePicker
                        id=format!("filter-{}-to", label.to_lowercase().replace(' ', "-"))
                        value=to
                        set_value=set_to
                        invalid=Signal::derive(move || problem.get().is_some())
                        disabled=Signal::derive(|| false)
                        open=popovers.1
                    />
                </div>
            </div>
            <p class="mt-2 text-sm text-error">{move || problem.get().unwrap_or_default()}</p>
        </div>
    }
}

/// Everything is a draft until Apply; the count answers the draft, not the table.
#[component]
pub fn FilterDialog(
    open: RwSignal<bool>,
    filters: RwSignal<Filters>,
    /// A non-admin owns everything they can see, so the owner item is hidden.
    #[prop(into)]
    is_admin: Signal<bool>,
    /// The rest of the query, so the count answers what Apply would list.
    #[prop(into)]
    base: Signal<Vec<(String, String)>>,
) -> impl IntoView {
    let draft = RwSignal::new(Filters::default());
    let matched = RwSignal::new(None::<u64>);
    let counting = RwSignal::new(false);
    let (generation, set_generation) = signal(0u32);
    let owner_search = RwSignal::new(String::new());
    let candidates = RwSignal::new(Vec::<String>::new());

    // A copy, so Cancel is simply not writing it back.
    Effect::new(move |_| {
        if open.get() {
            draft.set(filters.get_untracked());
            owner_search.set(String::new());
            candidates.set(Vec::new());
        }
    });

    // One count per pause in editing, and only the last of a burst survives.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let params = draft.get().to_params(local_offset_minutes());
        let mut query = base.get_untracked();
        query.extend(params);
        set_generation.update(|n| *n += 1);
        let mine = generation.get_untracked();
        counting.set(true);
        set_timeout(
            move || {
                if generation.try_get_untracked() != Some(mine) {
                    return;
                }
                spawn_local(async move {
                    let answer = api::count_urls(query, None).await;
                    if generation.try_get_untracked() != Some(mine) {
                        return;
                    }
                    matched.try_set(answer.ok().map(|counted| counted.total));
                    counting.try_set(false);
                });
            },
            SEARCH_DEBOUNCE,
        );
    });

    // `?q=` does not reach the admins bucket, so it is narrowed here.
    Effect::new(move |_| {
        let typed = owner_search.get().trim().to_lowercase();
        if typed.is_empty() {
            candidates.set(Vec::new());
            return;
        }
        spawn_local(async move {
            let params = vec![
                ("q".to_string(), typed.clone()),
                ("limit".to_string(), "10".to_string()),
            ];
            let Ok(page) = api::admin_users(params).await else {
                return;
            };
            let mut names: Vec<String> = page
                .admins
                .iter()
                .filter(|user| user.username.to_lowercase().contains(&typed))
                .chain(page.items.iter())
                .map(|user| user.username.clone())
                .collect();
            names.dedup();
            names.truncate(10);
            candidates.try_set(names);
        });
    });

    let owner_is = move |choice: &OwnerChoice| {
        std::mem::discriminant(&draft.get().owner) == std::mem::discriminant(choice)
    };
    let expiry_is = move |choice: &ExpiryChoice| {
        std::mem::discriminant(&draft.get().expiry) == std::mem::discriminant(choice)
    };
    let owner_names = Signal::derive(move || match draft.get().owner {
        OwnerChoice::Usernames(names) => names,
        _ => Vec::new(),
    });
    let set_owner_names = Callback::new(move |names: Vec<String>| {
        draft.update(|held| held.owner = OwnerChoice::Usernames(names));
    });
    let expiry_range = Signal::derive(move || match draft.get().expiry {
        ExpiryChoice::Between(range) => range,
        _ => DateRange::default(),
    });
    // Two per date item, since each picker owns its own calendar.
    let expiry_pops = (RwSignal::new(false), RwSignal::new(false));
    let last_hit_pops = (RwSignal::new(false), RwSignal::new(false));
    let updated_pops = (RwSignal::new(false), RwSignal::new(false));
    let created_pops = (RwSignal::new(false), RwSignal::new(false));
    let any_popover = move || {
        [
            expiry_pops.0,
            expiry_pops.1,
            last_hit_pops.0,
            last_hit_pops.1,
            updated_pops.0,
            updated_pops.1,
            created_pops.0,
            created_pops.1,
        ]
        .iter()
        .any(|open| open.get_untracked())
    };
    let first_field: NodeRef<leptos::html::Input> = NodeRef::new();

    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        request_animation_frame(move || {
            if let Some(input) = first_field.get_untracked() {
                let _ = input.focus();
            }
        });
    });

    let problem = Memo::new(move |_| draft.get().problem(&now_local()));
    let hits_problem = Memo::new(move |_| draft.get().hits_problem());
    let apply = move || {
        if problem.get_untracked().is_none() {
            filters.set(draft.get_untracked());
            open.set(false);
        }
    };
    let matched_text = move || match (counting.get(), matched.get()) {
        (true, _) => "counting…".to_string(),
        (false, Some(1)) => "1 link matches".to_string(),
        (false, Some(total)) => format!("{total} links match"),
        (false, None) => String::new(),
    };

    view! {
        <Show when=move || open.get()>
            <div
                class="flex fixed inset-0 z-50 justify-center items-center p-4 bg-black/50 motion-preset-fade motion-duration-200"
                role="dialog"
                aria-modal="true"
                on:click=move |ev| {
                    let backdrop = ev.target().map(wasm_bindgen::JsValue::from)
                        == ev.current_target().map(wasm_bindgen::JsValue::from);
                    if backdrop {
                        open.set(false);
                    }
                }
                on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                    if ev.key() == "Escape" && !any_popover() {
                        open.set(false);
                    }
                }
            >
                <div class="flex flex-col w-full max-w-3xl max-h-full rounded-lg border shadow-xl bg-base-100 border-base-content/10 motion-preset-slide-down motion-duration-200">
                    <div class="flex gap-3 justify-between items-baseline px-8 pt-8 pb-4 border-b border-base-content/10">
                        <h3 class="text-2xl font-semibold">"Advanced Filter"</h3>
                        <span class="text-sm opacity-70">{matched_text}</span>
                    </div>

                    <form
                        class="flex overflow-auto flex-col flex-1 gap-6 py-6 px-8"
                        id="advanced-filter"
                        novalidate
                        on:submit=move |ev: leptos::ev::SubmitEvent| {
                            ev.prevent_default();
                            apply();
                        }
                    >
                        <TextItem
                            label="Code contains"
                            placeholder="promo, then Enter"
                            values=Signal::derive(move || draft.get().codes)
                            set_values=Callback::new(move |values: Vec<String>| {
                                draft.update(|held| held.codes = values);
                            })
                            input_ref=first_field
                        />
                        <TextItem
                            label="Destination contains"
                            placeholder="example.com, then Enter"
                            values=Signal::derive(move || draft.get().urls)
                            set_values=Callback::new(move |values: Vec<String>| {
                                draft.update(|held| held.urls = values);
                            })
                        />

                        <Show when=move || is_admin.get()>
                            <div class="w-full">
                                <label class="mb-2 font-semibold label-text">"Owner"</label>
                                <div class="flex gap-4 items-center">
                                    <label class="flex gap-2 items-center cursor-pointer">
                                        <input
                                            type="radio"
                                            class="radio radio-sm"
                                            prop:checked=move || owner_is(&OwnerChoice::Any)
                                            on:change=move |_| {
                                                draft.update(|held| held.owner = OwnerChoice::Any)
                                            }
                                        />
                                        "Anyone"
                                    </label>
                                    <label class="flex gap-2 items-center cursor-pointer">
                                        <input
                                            type="radio"
                                            class="radio radio-sm"
                                            prop:checked=move || owner_is(&OwnerChoice::Mine)
                                            on:change=move |_| {
                                                draft.update(|held| held.owner = OwnerChoice::Mine)
                                            }
                                        />
                                        "Mine"
                                    </label>
                                    <label class="flex gap-2 items-center cursor-pointer">
                                        <input
                                            type="radio"
                                            class="radio radio-sm"
                                            prop:checked=move || owner_is(&OwnerChoice::Unowned)
                                            on:change=move |_| {
                                                draft.update(|held| held.owner = OwnerChoice::Unowned)
                                            }
                                        />
                                        "Nobody"
                                    </label>
                                    <label class="flex gap-2 items-center cursor-pointer">
                                        <input
                                            type="radio"
                                            class="radio radio-sm"
                                            prop:checked=move || {
                                                owner_is(&OwnerChoice::Usernames(Vec::new()))
                                            }
                                            on:change=move |_| {
                                                draft
                                                    .update(|held| {
                                                        held.owner = OwnerChoice::Usernames(Vec::new())
                                                    })
                                            }
                                        />
                                        "These accounts"
                                    </label>
                                </div>
                                <Show when=move || owner_is(&OwnerChoice::Usernames(Vec::new()))>
                                    <div class="mt-2">
                                        <input
                                            class=input_class(false)
                                            placeholder="Search accounts"
                                            autocomplete="off"
                                            prop:value=owner_search
                                            on:input=move |ev| owner_search.set(event_target_value(&ev))
                                        />
                                        <Show when=move || !candidates.get().is_empty()>
                                            <div class="flex flex-wrap gap-1 mt-2">
                                                {move || {
                                                    candidates
                                                        .get()
                                                        .into_iter()
                                                        .map(|name| {
                                                            let picked = name.clone();
                                                            view! {
                                                                <button
                                                                    type="button"
                                                                    class="badge badge-soft"
                                                                    on:click=move |_| {
                                                                        let mut next = owner_names.get_untracked();
                                                                        if !next.contains(&picked) {
                                                                            next.push(picked.clone());
                                                                            set_owner_names.run(next);
                                                                        }
                                                                        owner_search.set(String::new());
                                                                    }
                                                                >
                                                                    {name.clone()}
                                                                </button>
                                                            }
                                                        })
                                                        .collect_view()
                                                }}
                                            </div>
                                        </Show>
                                        <Chips values=owner_names set_values=set_owner_names />
                                    </div>
                                </Show>
                            </div>
                        </Show>

                        <div class="w-full">
                            <label class="mb-2 font-semibold label-text">"Expiry"</label>
                            <div class="flex gap-4 items-center">
                                <label class="flex gap-2 items-center cursor-pointer">
                                    <input
                                        type="radio"
                                        class="radio radio-sm"
                                        prop:checked=move || expiry_is(&ExpiryChoice::Any)
                                        on:change=move |_| {
                                            draft.update(|held| held.expiry = ExpiryChoice::Any)
                                        }
                                    />
                                    "Any"
                                </label>
                                <label class="flex gap-2 items-center cursor-pointer">
                                    <input
                                        type="radio"
                                        class="radio radio-sm"
                                        prop:checked=move || expiry_is(&ExpiryChoice::Never)
                                        on:change=move |_| {
                                            draft.update(|held| held.expiry = ExpiryChoice::Never)
                                        }
                                    />
                                    "Never expires"
                                </label>
                                <label class="flex gap-2 items-center cursor-pointer">
                                    <input
                                        type="radio"
                                        class="radio radio-sm"
                                        prop:checked=move || {
                                            expiry_is(&ExpiryChoice::Between(DateRange::default()))
                                        }
                                        on:change=move |_| {
                                            draft
                                                .update(|held| {
                                                    held.expiry = ExpiryChoice::Between(DateRange::default())
                                                })
                                        }
                                    />
                                    "Expires between"
                                </label>
                            </div>
                            <Show when=move || {
                                expiry_is(&ExpiryChoice::Between(DateRange::default()))
                            }>
                                <div class="mt-2">
                                    <DateRangeItem
                                        label="Expires"
                                        range=expiry_range
                                        set_range=Callback::new(move |range: DateRange| {
                                            draft
                                                .update(|held| {
                                                    held.expiry = ExpiryChoice::Between(range)
                                                });
                                        })
                                        popovers=expiry_pops
                                        problem=Signal::derive(move || {
                                            expiry_range.get().problem(None)
                                        })
                                    />
                                </div>
                            </Show>
                        </div>

                        <div class="w-full">
                            <label class="mb-2 font-semibold label-text">"Hits"</label>
                            <div class="flex gap-2 items-center">
                                <input
                                    type="number"
                                    min="0"
                                    class=move || {
                                        format!(
                                            "{} {SPINNERLESS}",
                                            input_class(hits_problem.get().is_some()),
                                        )
                                    }
                                    placeholder="from"
                                    prop:value=move || draft.get().hits_min
                                    on:input=move |ev| {
                                        draft.update(|held| held.hits_min = event_target_value(&ev))
                                    }
                                />
                                <span class="opacity-60">"to"</span>
                                <input
                                    type="number"
                                    min="0"
                                    class=move || {
                                        format!(
                                            "{} {SPINNERLESS}",
                                            input_class(hits_problem.get().is_some()),
                                        )
                                    }
                                    placeholder="to"
                                    prop:value=move || draft.get().hits_max
                                    on:input=move |ev| {
                                        draft.update(|held| held.hits_max = event_target_value(&ev))
                                    }
                                />
                            </div>
                            <p class="mt-2 text-sm text-error">
                                {move || hits_problem.get().unwrap_or_default()}
                            </p>
                        </div>

                        <DateRangeItem
                            label="Last hit"
                            range=Signal::derive(move || draft.get().last_hit)
                            set_range=Callback::new(move |range: DateRange| {
                                draft.update(|held| held.last_hit = range);
                            })
                            popovers=last_hit_pops
                            problem=Signal::derive(move || {
                                draft.get().last_hit.problem(Some(&now_local()))
                            })
                        />
                        <DateRangeItem
                            label="Updated"
                            range=Signal::derive(move || draft.get().updated)
                            set_range=Callback::new(move |range: DateRange| {
                                draft.update(|held| held.updated = range);
                            })
                            popovers=updated_pops
                            problem=Signal::derive(move || {
                                draft.get().updated.problem(Some(&now_local()))
                            })
                        />
                        <DateRangeItem
                            label="Created"
                            range=Signal::derive(move || draft.get().created)
                            set_range=Callback::new(move |range: DateRange| {
                                draft.update(|held| held.created = range);
                            })
                            popovers=created_pops
                            problem=Signal::derive(move || {
                                draft.get().created.problem(Some(&now_local()))
                            })
                        />
                    </form>

                    <div class="flex gap-2 justify-end px-8 pt-4 pb-8 border-t border-base-content/10">
                        <button
                            class="btn btn-text"
                            type="button"
                            on:click=move |_| draft.set(Filters::default())
                        >
                            "Clear all"
                        </button>
                        <button
                            class="btn btn-text"
                            type="button"
                            on:click=move |_| open.set(false)
                        >
                            "Cancel"
                        </button>
                        <button
                            class="btn btn-primary"
                            type="submit"
                            form="advanced-filter"
                            disabled=move || problem.get().is_some()
                        >
                            "Apply"
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}
