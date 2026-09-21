use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::{UrlEntry, UrlUpsertRequest, validate_code, validate_url};
use wasm_bindgen::JsCast;

use crate::api;
use crate::auth::use_auth;
use crate::clipboard::{copy, origin, short_link};
use crate::components::avatar::{Avatar, usable_url};
use crate::components::confirm::ConfirmDialog;
use crate::components::conflict::ReplacementDetails;
use crate::components::conflict::other_owner;
use crate::components::datepicker::DateTimePicker;
use crate::components::tooltip::Tooltip;
use crate::datetime::{
    from_display, from_rfc3339, is_future, local_offset_minutes, to_display, to_rfc3339,
};
use crate::list::{
    GHOST_DELAY, GHOST_ROWS, GhostRow, LOAD_MORE_MARGIN, SEARCH_DEBOUNCE, Sort, SortHeader,
    all_selected, list_params, short_datetime,
};
use crate::toast::use_toasts;
use crate::ui::row_input_class;

/// Most recently touched first, which is also what the server applies when no
/// sort is sent — so cycling a column back to "off" lands here.
pub const DEFAULT_SORT: Sort = Sort {
    field: "updated_at",
    desc: true,
};

/// What the delete dialog says, for one row or for a whole selection.
fn delete_message(codes: &[String]) -> String {
    match codes {
        [] => String::new(),
        [one] => format!("Delete /{one}? This cannot be undone."),
        many => format!("Delete {} links? This cannot be undone.", many.len()),
    }
}

/// What the takeover dialog says. The owned links are why it appears, but the
/// action covers the rest too — counting only takeovers would describe less
/// than the button is about to do.
fn claim_message(links: &[(String, Option<String>)]) -> String {
    let taken = links.iter().filter(|(_, owner)| owner.is_some()).count();
    match links {
        [] => String::new(),
        [(code, Some(owner))] => {
            format!("Take /{code} from {owner}? They lose it from their list.")
        }
        [(code, None)] => format!("Claim /{code}? Nobody owns it."),
        all if all.len() == taken => {
            format!("Take {taken} links from their owners? They lose them from their lists.")
        }
        all => format!(
            "Claim {} links? {taken} of them are taken from their owners, who lose them from their lists.",
            all.len(),
        ),
    }
}

/// A column header that sorts. Owns the toggle rather than taking a callback,
/// since the only thing it does is rewrite the shared `Sort`.
#[component]
pub fn MyUrls() -> impl IntoView {
    let auth = use_auth();
    let toasts = use_toasts();

    let (items, set_items) = signal(Vec::<UrlEntry>::new());
    let (total, set_total) = signal(0u64);
    let (page, set_page) = signal(0u64);
    let (loaded_once, set_loaded_once) = signal(false);
    let (loading, set_loading) = signal(false);
    // Raw keystrokes, and the value that actually reaches the server.
    let (search, set_search) = signal(String::new());
    let (debounced, set_debounced) = signal(String::new());
    let (keystroke, set_keystroke) = signal(0u32);
    let sort = RwSignal::new(Some(DEFAULT_SORT));
    let selected = RwSignal::new(HashSet::<String>::new());
    // The row being edited, keyed by the code it had when editing began — a
    // rename changes the code, so the original is what identifies the row.
    let editing = RwSignal::new(Option::<String>::None);
    let (draft_code, set_draft_code) = signal(String::new());
    let (draft_url, set_draft_url) = signal(String::new());
    let (draft_expiry, set_draft_expiry) = signal(String::new());
    // The row being saved, the link it collided with, and whether landing on
    // it would destroy that link rather than replace this one.
    let (conflict, set_conflict) = signal(None::<(String, UrlEntry, bool)>);
    let reset_hits = RwSignal::new(false);
    let claim_owner = RwSignal::new(false);
    let conflict_open = RwSignal::new(false);
    // Which field failed local validation, if any. Only a red border: the rule
    // it broke is the same one the placeholder implies, and a message per row
    // would push the table around.
    let (invalid_field, set_invalid_field) = signal(Option::<&'static str>::None);
    let (saving, set_saving) = signal(false);
    let pending_delete = RwSignal::new(Vec::<String>::new());
    let confirm_open = RwSignal::new(false);
    // The link a takeover is waiting on. Claiming an unowned one needs no
    // confirmation — nothing is taken from anybody — but taking one off its
    // owner does.
    let pending_claim = RwSignal::new(Vec::<UrlEntry>::new());
    let claim_open = RwSignal::new(false);
    // The request in flight, so a new search can cancel it. Held locally
    // because a JS object is neither Send nor Sync.
    let inflight = StoredValue::new_local(Option::<web_sys::AbortController>::None);
    // Guards against a cancelled or overtaken response writing stale rows.
    let (generation, set_generation) = signal(0u32);
    // Raised only once a load has been slow enough to be worth showing.
    let (slow, set_slow) = signal(false);

    // Reads every input untracked so callers decide when it runs: the effect
    // below re-runs it on a new search or sort, the scroller on a new page.
    let fetch = move |index: u64, append: bool| {
        if auth.user.get_untracked().is_none() {
            return;
        }
        let params = list_params(index, &debounced.get_untracked(), sort.get_untracked());

        // Cancel whatever is still in flight: the answer is about to be wrong.
        inflight.update_value(|slot| {
            if let Some(previous) = slot.take() {
                previous.abort();
            }
        });
        let controller = web_sys::AbortController::new().ok();
        let signal = controller.as_ref().map(|c| c.signal());
        inflight.set_value(controller);

        set_generation.update(|n| *n += 1);
        let mine = generation.get_untracked();
        set_loading.set(true);
        set_slow.set(false);
        // `try_` throughout: a timer or a request can outlive the page that
        // started it, and writing a signal whose owner has been disposed panics
        // — which in wasm is fatal to the whole app, not just to this page.
        set_timeout(
            move || {
                // Still the current request, and still waiting.
                if generation.try_get_untracked() == Some(mine)
                    && loading.try_get_untracked() == Some(true)
                {
                    set_slow.try_set(true);
                }
            },
            GHOST_DELAY,
        );
        spawn_local(async move {
            let result = api::list_urls(params, signal.as_ref()).await;
            // A newer request started while this one was out — including the
            // one that aborted it, whose error is not worth showing. A page
            // that is gone entirely reads as `None` and stops here too.
            if generation.try_get_untracked() != Some(mine) {
                return;
            }
            match result {
                Ok(response) => {
                    set_total.try_set(response.total);
                    if append {
                        set_items.try_update(|rows| rows.extend(response.items));
                    } else {
                        set_items.try_set(response.items);
                    }
                    set_page.try_set(index);
                }
                Err(err) => toasts.error(err.message),
            }
            set_loading.try_set(false);
            set_slow.try_set(false);
            set_loaded_once.try_set(true);
        });
    };

    // Start over whenever the session resolves or the query changes — a
    // different sort or search invalidates every page already accumulated, and
    // an open editor refers to a row that may not survive it.
    Effect::new(move |_| {
        auth.user.get();
        debounced.get();
        sort.get();
        editing.set(None);
        // The rows are about to change, and a tick against a row that is no
        // longer listed would delete something the user cannot see.
        selected.set(HashSet::new());
        set_items.set(Vec::new());
        fetch(0, false);
    });

    let more_to_load = move || (items.get().len() as u64) < total.get();
    // A fresh query has nothing to show yet; appending keeps the rows visible.
    // Gated on `slow` so a quick response goes straight to rows.
    let showing_ghosts = move || slow.get() && loading.get() && items.get().is_empty();

    let on_scroll = move |ev: leptos::ev::Event| {
        let Some(el) = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let bottom = f64::from(el.scroll_top() + el.client_height());
        let reached = bottom >= f64::from(el.scroll_height()) - LOAD_MORE_MARGIN;
        if reached && !loading.get_untracked() && more_to_load() {
            fetch(page.get_untracked() + 1, true);
        }
    };

    let on_search = move |ev| {
        set_search.set(event_target_value(&ev));
        set_keystroke.update(|n| *n += 1);
        // Only the last keystroke of a burst survives its own timer.
        let mine = keystroke.get_untracked();
        set_timeout(
            move || {
                if keystroke.try_get_untracked() == Some(mine)
                    && let Some(typed) = search.try_get_untracked()
                {
                    set_debounced.try_set(typed);
                }
            },
            SEARCH_DEBOUNCE,
        );
    };

    // Deletes each code in turn and drops its row. Refetching instead would
    // throw away every page scrolled so far.
    // Clearing skips the debounce: there is nothing more to type, so waiting
    // would only delay the results the user just asked for.
    let clear_search = move |_| {
        set_search.set(String::new());
        set_keystroke.update(|n| *n += 1);
        set_debounced.set(String::new());
    };

    // One request per link, in sequence. There is no bulk endpoint for links
    // and none is needed: unlike the admin chain, links do not cascade — taking
    // /a has no bearing on /b — so a refusal on one says nothing about the
    // rest, and reporting them separately is the honest thing to do.
    let claim_many = move |codes: Vec<String>| {
        spawn_local(async move {
            let mut taken = 0usize;
            let mut failed = Vec::new();
            let mut last_owner = String::new();
            for code in codes {
                match api::claim_url(&code).await {
                    Ok(updated) => {
                        last_owner = updated
                            .owner
                            .as_ref()
                            .and_then(|o| o.username.clone())
                            .unwrap_or_default();
                        set_items.update(|rows| {
                            if let Some(row) = rows.iter_mut().find(|row| row.code == code) {
                                *row = updated;
                            }
                        });
                        selected.update(|set| {
                            set.remove(&code);
                        });
                        taken += 1;
                    }
                    Err(err) => failed.push(format!("/{code}: {}", err.message)),
                }
            }
            match (taken, failed.as_slice()) {
                (0, []) => {}
                (1, []) => toasts.success(format!("1 link is {last_owner}'s now")),
                (n, []) => toasts.success(format!("{n} links are {last_owner}'s now")),
                (_, problems) => toasts.error(format!("Could not claim {}", problems.join("; "))),
            }
        });
    };

    let claim_confirmed = Callback::new(move |()| {
        let codes = pending_claim
            .get_untracked()
            .into_iter()
            .map(|entry| entry.code)
            .collect();
        claim_many(codes);
    });

    // Asks first only when something is being taken from somebody. A link
    // nobody owns is nobody's to defend, and a dialog about it would be a
    // question with one answer.
    let ask_claim = move |entries: Vec<UrlEntry>| {
        if entries.iter().any(|entry| entry.owner.is_some()) {
            pending_claim.set(entries);
            claim_open.set(true);
            return;
        }
        claim_many(entries.into_iter().map(|entry| entry.code).collect());
    };

    let delete_confirmed = Callback::new(move |()| {
        let codes = pending_delete.get_untracked();
        spawn_local(async move {
            let mut failed = Vec::new();
            let mut deleted = 0usize;
            for code in codes {
                match api::delete_url(&code).await {
                    Ok(()) => {
                        set_items.update(|rows| rows.retain(|row| row.code != code));
                        set_total.update(|t| *t = t.saturating_sub(1));
                        selected.update(|set| {
                            set.remove(&code);
                        });
                        deleted += 1;
                    }
                    Err(err) => failed.push(format!("/{code}: {}", err.message)),
                }
            }
            match (deleted, failed.as_slice()) {
                (0, []) => {}
                (n, []) => toasts.success(format!("Deleted {n} link(s)")),
                (_, problems) => toasts.error(format!("Could not delete {}", problems.join("; "))),
            }
        });
    });

    let ask_delete = move |codes: Vec<String>| {
        pending_delete.set(codes);
        confirm_open.set(true);
    };

    let begin_edit = move |entry: UrlEntry| {
        set_draft_code.set(entry.code.clone());
        set_draft_url.set(entry.url.clone());
        set_draft_expiry.set(
            entry
                .expires_at
                .as_deref()
                .and_then(|stamp| from_rfc3339(stamp, local_offset_minutes()))
                .and_then(|local| to_display(&local))
                .unwrap_or_default(),
        );
        set_invalid_field.set(None);
        editing.set(Some(entry.code));
    };

    let cancel_edit = move || {
        editing.set(None);
        set_invalid_field.set(None);
    };

    let save_edit = move |original: String, overwrite: bool| {
        let code = draft_code.get_untracked().trim().to_string();
        let url = draft_url.get_untracked().trim().to_string();
        // The same rules the server enforces, checked here so a typo costs no
        // round trip.
        if validate_code(&code).is_err() {
            set_invalid_field.set(Some("code"));
            return;
        }
        if validate_url(&url).is_err() {
            set_invalid_field.set(Some("url"));
            return;
        }
        // Empty means "no expiry", which the server reads as a removal — so a
        // link can be freed as well as dated from the same box.
        let shown = draft_expiry.get_untracked();
        let expires_at = match shown.trim() {
            "" => String::new(),
            shown => {
                let stamp = from_display(shown)
                    .and_then(|local| to_rfc3339(&local, local_offset_minutes()))
                    .filter(|stamp| is_future(stamp));
                let Some(stamp) = stamp else {
                    set_invalid_field.set(Some("expires_at"));
                    return;
                };
                stamp
            }
        };
        set_invalid_field.set(None);
        set_saving.set(true);
        let reset = overwrite && reset_hits.get_untracked();
        let claim = overwrite && claim_owner.get_untracked();
        let renaming = code != original;
        spawn_local(async move {
            let request = UrlUpsertRequest {
                code,
                url,
                expires_at: Some(expires_at),
                overwrite: overwrite.then_some(true),
                reset_hits: reset.then_some(true),
                claim: claim.then_some(true),
            };
            match api::update_url(&original, &request).await {
                Ok(updated) => {
                    let renamed = updated.code.clone();
                    set_items.update(|rows| {
                        if let Some(row) = rows.iter_mut().find(|row| row.code == original) {
                            *row = updated;
                        }
                    });
                    // A rename moves the row's identity, so a tick against the
                    // old code would otherwise point at nothing.
                    selected.update(|set| {
                        if set.remove(&original) {
                            set.insert(renamed);
                        }
                    });
                    editing.set(None);
                    set_invalid_field.set(None);
                    toasts.success("Link saved");
                }
                // A code the caller already owns is an offer, not a refusal:
                // the dialog says what would go and sends this again.
                Err(err) => match err.conflict {
                    Some(existing) => {
                        reset_hits.set(false);
                        claim_owner.set(false);
                        set_conflict.set(Some((original, *existing, renaming)));
                        conflict_open.set(true);
                    }
                    None => toasts.error(err.message),
                },
            }
            set_saving.set(false);
        });
    };

    // Enter commits the row, Escape abandons it — the two keys a keyboard user
    // reaches for once a table cell has turned into an input.
    let edit_keys = move |ev: &leptos::ev::KeyboardEvent, original: &str| match ev.key().as_str() {
        "Enter" => save_edit(original.to_string(), false),
        "Escape" => cancel_edit(),
        _ => {}
    };

    let visible_codes = move || {
        items
            .get()
            .iter()
            .map(|entry| entry.code.clone())
            .collect::<Vec<_>>()
    };
    let toggle_all = move |_| {
        let codes = visible_codes();
        let clear = all_selected(&codes, &selected.get_untracked());
        selected.update(|set| {
            for code in codes {
                if clear {
                    set.remove(&code);
                } else {
                    set.insert(code);
                }
            }
        });
    };

    let empty = move || loaded_once.get() && !loading.get() && items.get().is_empty();
    let selected_count = move || selected.get().len();
    // Kept out of the view: leptosfmt reads the `>` of a comparison inside an
    // attribute as the element's closing bracket and mangles the markup.
    let has_selection = move || selected_count() > 0;
    // The ticked links worth claiming: the ones this admin does not already
    // own. Whether the server will allow each is a question about the owner's
    // place in the chain, which this page cannot answer — so the offer covers
    // what is plausibly claimable and the refusals come back per link.
    let claimable_selection = move || {
        let me = auth
            .user
            .get()
            .map(|user| user.username)
            .unwrap_or_default();
        let ticked = selected.get();
        items
            .get()
            .into_iter()
            .filter(|row| ticked.contains(&row.code))
            .filter(|row| {
                row.owner
                    .as_ref()
                    .and_then(|owner| owner.username.as_deref())
                    != Some(me.as_str())
            })
            .collect::<Vec<_>>()
    };
    let can_claim = move || auth.is_admin() && !claimable_selection().is_empty();
    // Checkbox, code, destination, hits, last hit, expires, created, updated,
    // actions — plus the admin-only owner column.
    let column_count = move || if auth.is_admin() { 10 } else { 9 };

    view! {
        <Show
            when=move || auth.user.get().is_some()
            fallback=|| {
                view! {
                    <div class="py-16 text-center">
                        <p class="mb-4 text-lg">"Sign in to see the links you own."</p>
                        <a href="#/login" class="gap-2 btn btn-primary">
                            <span class="icon-[tabler--login] size-5"></span>
                            "Sign in"
                        </a>
                    </div>
                }
            }
        >
            <div class="flex flex-col gap-4 motion-preset-fade motion-duration-500">
                <div class="flex flex-wrap gap-3 justify-between items-center">
                    <h2 class="text-3xl font-bold">
                        {move || if auth.is_admin() { "All URLs" } else { "My URLs" }}
                    </h2>
                    <div class="flex gap-2 items-center">
                        <Show when=can_claim>
                            <button
                                class="gap-2 btn"
                                on:click=move |_| ask_claim(claimable_selection())
                            >
                                <span class="icon-[tabler--hand-grab] size-4"></span>
                                {move || format!("Claim {}", claimable_selection().len())}
                            </button>
                        </Show>
                        <Show when=has_selection>
                            <button
                                class="gap-2 btn btn-error"
                                on:click=move |_| {
                                    ask_delete(selected.get_untracked().into_iter().collect())
                                }
                            >
                                <span class="icon-[tabler--trash] size-4"></span>
                                {move || format!("Delete {}", selected_count())}
                            </button>
                        </Show>
                        <div class="relative">
                            <input
                                class="pr-9 w-64 max-w-full input"
                                placeholder="Search code or url"
                                aria-label="Search links"
                                prop:value=search
                                on:input=on_search
                            />
                            <Show when=move || !search.get().is_empty()>
                                <button
                                    class="flex absolute right-2 top-1/2 justify-center items-center rounded opacity-60 -translate-y-1/2 hover:opacity-100 size-6 hover:bg-base-200"
                                    aria-label="Clear search"
                                    on:click=clear_search
                                >
                                    <span class="icon-[tabler--x] size-4"></span>
                                </button>
                            </Show>
                        </div>
                    </div>
                </div>

                <div
                    class="overflow-auto rounded-lg border h-[calc(100vh-16rem)] border-base-content/10"
                    on:scroll=on_scroll
                >
                    <table class="table table-fixed table-pinned min-w-[84rem] [&_thead_tr]:border-b-0 [&_td]:px-3">
                        <thead class="sticky top-0 z-10 bg-base-200">
                            <tr>
                                <th class="px-3 w-10">
                                    <input
                                        type="checkbox"
                                        class="checkbox checkbox-sm"
                                        aria-label="Select all"
                                        prop:checked=move || {
                                            all_selected(&visible_codes(), &selected.get())
                                        }
                                        on:change=toggle_all
                                    />
                                </th>
                                <SortHeader field="code" label="Code" sort=sort width="w-32" />
                                <SortHeader field="url" label="Destination" sort=sort />
                                <Show when=move || auth.is_admin()>
                                    <th class="px-3 w-36">"Owner"</th>
                                </Show>
                                <SortHeader field="hits" label="Hits" sort=sort width="w-24" />
                                <SortHeader
                                    field="last_hit_at"
                                    label="Last hit"
                                    sort=sort
                                    width="w-36"
                                />
                                <SortHeader
                                    field="expires_at"
                                    label="Expires"
                                    sort=sort
                                    width="w-56"
                                />
                                <SortHeader
                                    field="created_at"
                                    label="Created"
                                    sort=sort
                                    width="w-36"
                                />
                                <SortHeader
                                    field="updated_at"
                                    label="Updated"
                                    sort=sort
                                    width="w-36"
                                />
                                <th class="px-3 w-32"></th>
                            </tr>
                        </thead>
                        <tbody>
                            <Show when=showing_ghosts>
                                {move || {
                                    (0..GHOST_ROWS)
                                        .map(|_| view! { <GhostRow columns=column_count() /> })
                                        .collect_view()
                                }}
                            </Show>

                            <For
                                each=move || items.get()
                                key=|entry| {
                                    format!(
                                        "{}@{}",
                                        entry.code,
                                        entry.updated_at.clone().unwrap_or_default(),
                                    )
                                }
                                let:entry
                            >
                                {
                                    let row_code = StoredValue::new(entry.code.clone());
                                    let code = StoredValue::new(entry.code.clone());
                                    let url = StoredValue::new(entry.url.clone());
                                    let for_edit = StoredValue::new(entry.clone());
                                    let owner_name = entry
                                        .owner
                                        .as_ref()
                                        .and_then(|o| o.username.clone())
                                        .unwrap_or_default();
                                    let owner_avatar = entry
                                        .owner
                                        .as_ref()
                                        .and_then(|o| usable_url(o.avatar_url.as_deref()));
                                    let has_owner = entry.owner.is_some();
                                    let owned_by_me = auth
                                        .user
                                        .get_untracked()
                                        .is_some_and(|me| me.username == owner_name);
                                    let claimable = auth.is_admin() && !owned_by_me;
                                    let taking_over = has_owner;
                                    let hits = entry.hits;
                                    let last = short_datetime(entry.last_hit_at.as_ref());
                                    let expires = short_datetime(entry.expires_at.as_ref());
                                    let created = short_datetime(entry.created_at.as_ref());
                                    let updated = short_datetime(entry.updated_at.as_ref());
                                    let is_editing = move || {
                                        row_code
                                            .with_value(|code| {
                                                editing.get().as_deref() == Some(code.as_str())
                                            })
                                    };
                                    view! {
                                        <tr>
                                            <td>
                                                <input
                                                    type="checkbox"
                                                    class="checkbox checkbox-sm"
                                                    aria-label=move || format!("Select {}", code.get_value())
                                                    prop:checked=move || {
                                                        code.with_value(|c| selected.get().contains(c))
                                                    }
                                                    on:change=move |_| {
                                                        let this = code.get_value();
                                                        selected
                                                            .update(|set| {
                                                                if !set.remove(&this) {
                                                                    set.insert(this);
                                                                }
                                                            });
                                                    }
                                                />
                                            </td>

                                            <Show
                                                when=is_editing
                                                fallback=move || {
                                                    view! {
                                                        <td class="font-mono">
                                                            <Tooltip
                                                                text=code.get_value()
                                                                class="block cursor-pointer truncate"
                                                            >
                                                                <span on:click=move |_| {
                                                                    let link = short_link(&origin(), &code.get_value());
                                                                    copy(link.clone());
                                                                    toasts.success(format!("Copied {link}"));
                                                                }>{code.get_value()}</span>
                                                            </Tooltip>
                                                        </td>
                                                        <td>
                                                            <Tooltip text=url.get_value()>
                                                                <a
                                                                    href=url.get_value()
                                                                    target="_blank"
                                                                    rel="noreferrer"
                                                                    class="link"
                                                                >
                                                                    {url.get_value()}
                                                                </a>
                                                            </Tooltip>
                                                        </td>
                                                    }
                                                }
                                            >
                                                <td>
                                                    <input
                                                        class=move || {
                                                            format!(
                                                                "w-full font-mono {}",
                                                                row_input_class(invalid_field.get() == Some("code")),
                                                            )
                                                        }
                                                        aria-label="Code"
                                                        prop:value=draft_code
                                                        on:input=move |ev| {
                                                            set_draft_code.set(event_target_value(&ev));
                                                            set_invalid_field.set(None);
                                                        }
                                                        on:keydown=move |ev| {
                                                            code.with_value(|c| edit_keys(&ev, c))
                                                        }
                                                    />
                                                </td>
                                                <td>
                                                    <input
                                                        class=move || {
                                                            format!(
                                                                "w-full {}",
                                                                row_input_class(invalid_field.get() == Some("url")),
                                                            )
                                                        }
                                                        aria-label="Destination"
                                                        prop:value=draft_url
                                                        on:input=move |ev| {
                                                            set_draft_url.set(event_target_value(&ev));
                                                            set_invalid_field.set(None);
                                                        }
                                                        on:keydown=move |ev| {
                                                            code.with_value(|c| edit_keys(&ev, c))
                                                        }
                                                    />
                                                </td>
                                            </Show>

                                            <Show when=move || auth.is_admin()>
                                                <td>
                                                    {if has_owner {
                                                        let name = owner_name.clone();
                                                        view! {
                                                            <span class="flex gap-2 items-center min-w-0">
                                                                <span class="shrink-0">
                                                                    <Avatar
                                                                        url=owner_avatar.clone()
                                                                        name=name.clone()
                                                                        size="size-6"
                                                                    />
                                                                </span>
                                                                <Tooltip
                                                                    text=name.clone()
                                                                    class="block opacity-70 truncate"
                                                                >
                                                                    {name.clone()}
                                                                </Tooltip>
                                                            </span>
                                                        }
                                                            .into_any()
                                                    } else {
                                                        view! {
                                                            <span class="badge badge-soft badge-sm">"None"</span>
                                                        }
                                                            .into_any()
                                                    }}
                                                </td>
                                            </Show>
                                            <td>{hits}</td>
                                            <td class="whitespace-nowrap opacity-70">{last}</td>
                                            <Show
                                                when=is_editing
                                                fallback={
                                                    let expires = expires.clone();
                                                    move || {
                                                        view! {
                                                            <td class="whitespace-nowrap opacity-70">
                                                                {expires.clone()}
                                                            </td>
                                                        }
                                                    }
                                                }
                                            >
                                                <td>
                                                    <DateTimePicker
                                                        id=row_code.with_value(|c| format!("expires-{c}"))
                                                        value=draft_expiry
                                                        set_value=set_draft_expiry
                                                        invalid=Signal::derive(move || {
                                                            invalid_field.get() == Some("expires_at")
                                                        })
                                                        disabled=Signal::derive(move || saving.get())
                                                        small=true
                                                        on_keydown=Callback::new(move |
                                                            ev: leptos::ev::KeyboardEvent|
                                                        { code.with_value(|c| edit_keys(&ev, c)) })
                                                    />
                                                </td>
                                            </Show>

                                            <td class="whitespace-nowrap opacity-70">{created}</td>
                                            <td class="whitespace-nowrap opacity-70">{updated}</td>
                                            <td class="whitespace-nowrap">
                                                <span class="flex gap-1 justify-end">
                                                    <Show
                                                        when=is_editing
                                                        fallback=move || {
                                                            view! {
                                                                <Show when=move || claimable>
                                                                    <Tooltip
                                                                        text=if taking_over { "Take over" } else { "Claim" }
                                                                        class="inline-flex"
                                                                        only_when_clipped=false
                                                                    >
                                                                        <button
                                                                            class="btn btn-text btn-sm btn-square"
                                                                            aria-label="Claim link"
                                                                            on:click=move |_| { ask_claim(vec![for_edit.get_value()]) }
                                                                        >
                                                                            <span class="icon-[tabler--hand-grab] size-4"></span>
                                                                        </button>
                                                                    </Tooltip>
                                                                </Show>
                                                                <Tooltip
                                                                    text="Edit"
                                                                    class="inline-flex"
                                                                    only_when_clipped=false
                                                                >
                                                                    <button
                                                                        class="btn btn-text btn-sm btn-square"
                                                                        aria-label="Edit link"
                                                                        on:click=move |_| begin_edit(for_edit.get_value())
                                                                    >
                                                                        <span class="icon-[tabler--pencil] size-4"></span>
                                                                    </button>
                                                                </Tooltip>
                                                                <Tooltip
                                                                    text="Delete"
                                                                    class="inline-flex"
                                                                    only_when_clipped=false
                                                                >
                                                                    <button
                                                                        class="btn btn-text btn-sm btn-square text-error"
                                                                        aria-label="Delete link"
                                                                        on:click=move |_| ask_delete(vec![code.get_value()])
                                                                    >
                                                                        <span class="icon-[tabler--trash] size-4"></span>
                                                                    </button>
                                                                </Tooltip>
                                                            }
                                                        }
                                                    >
                                                        <Tooltip
                                                            text="Save"
                                                            class="inline-flex"
                                                            only_when_clipped=false
                                                        >
                                                            <button
                                                                class="btn btn-primary btn-sm btn-square"
                                                                aria-label="Save changes"
                                                                disabled=move || saving.get()
                                                                on:click=move |_| { save_edit(code.get_value(), false) }
                                                            >
                                                                <span class="icon-[tabler--check] size-4"></span>
                                                            </button>
                                                        </Tooltip>
                                                        <Tooltip
                                                            text="Cancel"
                                                            class="inline-flex"
                                                            only_when_clipped=false
                                                        >
                                                            <button
                                                                class="btn btn-text btn-sm btn-square"
                                                                aria-label="Cancel editing"
                                                                on:click=move |_| cancel_edit()
                                                            >
                                                                <span class="icon-[tabler--x] size-4"></span>
                                                            </button>
                                                        </Tooltip>
                                                    </Show>
                                                </span>
                                            </td>
                                        </tr>
                                    }
                                }
                            </For>
                        </tbody>
                    </table>

                    <Show when=move || loading.get() && !showing_ghosts()>
                        <p class="flex gap-2 justify-center items-center py-4 text-base-content/60">
                            <span class="loading loading-spinner loading-sm"></span>
                            "Loading…"
                        </p>
                    </Show>

                    <Show when=empty>
                        <p class="py-12 text-center text-base-content/60">
                            {move || {
                                if search.get().trim().is_empty() {
                                    "No links yet. Create one on the Shorten page."
                                } else {
                                    "No links match that search."
                                }
                            }}
                        </p>
                    </Show>
                </div>

                <span class="text-sm opacity-70">
                    {move || {
                        let shown = items.get().len();
                        match total.get() {
                            0 => String::new(),
                            n if n as usize == shown => format!("{n} link(s)"),
                            n => format!("{shown} of {n} links"),
                        }
                    }}
                </span>
            </div>

            <ConfirmDialog
                open=claim_open
                title="Take over link"
                message=Signal::derive(move || {
                    let links: Vec<(String, Option<String>)> = pending_claim
                        .get()
                        .into_iter()
                        .map(|entry| {
                            let owner = entry
                                .owner
                                .as_ref()
                                .and_then(|owner| owner.username.clone());
                            (entry.code, owner)
                        })
                        .collect();
                    claim_message(&links)
                })
                confirm_label="Take over"
                on_confirm=claim_confirmed
            />

            <ConfirmDialog
                open=confirm_open
                title="Delete links"
                message=Signal::derive(move || delete_message(&pending_delete.get()))
                confirm_label="Delete"
                on_confirm=delete_confirmed
            />

            <ConfirmDialog
                open=conflict_open
                title=Signal::derive(move || {
                    match conflict.get() {
                        Some((.., true)) => "Replace the other link?".to_string(),
                        _ => "Replace this link?".to_string(),
                    }
                })
                message=Signal::derive(move || {
                    conflict
                        .get()
                        .map(|(_, existing, renaming)| {
                            match (other_owner(&existing, &auth), renaming) {
                                (Some(name), true) => {
                                    format!(
                                        "/{} belongs to {name}. Moving this link onto it deletes theirs.",
                                        existing.code,
                                    )
                                }
                                (Some(name), false) => {
                                    format!("/{} belongs to {name}.", existing.code)
                                }
                                (None, true) => {
                                    format!(
                                        "/{} already exists. Moving this link onto it deletes it.",
                                        existing.code,
                                    )
                                }
                                (None, false) => format!("You already use /{}.", existing.code),
                            }
                        })
                        .unwrap_or_default()
                })
                confirm_label="Replace"
                confirm_class=Signal::derive(move || {
                    match conflict.get() {
                        Some((.., true)) => "btn-error".to_string(),
                        _ => "btn-primary".to_string(),
                    }
                })
                extra=ViewFn::from(move || {
                    conflict
                        .get()
                        .map(|(_, existing, renaming)| {
                            let owner = other_owner(&existing, &auth)
                                .and_then(|_| existing.owner.clone());
                            let claim = (!renaming && owner.is_some()).then_some(claim_owner);
                            view! {
                                <ReplacementDetails
                                    existing=existing
                                    url=draft_url
                                    expires=draft_expiry
                                    reset_hits=reset_hits
                                    claim=claim
                                    other_owner=owner
                                />
                            }
                        })
                })
                on_confirm=Callback::new(move |_| {
                    if let Some((original, ..)) = conflict.get_untracked() {
                        save_edit(original, true);
                    }
                })
            />
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_all_needs_something_to_select() {
        let codes = vec!["a".to_string(), "b".to_string()];
        let mut chosen = HashSet::new();
        assert!(!all_selected(&codes, &chosen));
        chosen.insert("a".to_string());
        assert!(!all_selected(&codes, &chosen), "one of two is not all");
        chosen.insert("b".to_string());
        assert!(all_selected(&codes, &chosen));
        // An empty table must not show a ticked box.
        assert!(!all_selected(&[], &chosen));
    }

    /// The prompt is about what is taken from somebody, so a selection that is
    /// half unowned counts the half that has an owner.
    #[test]
    fn the_takeover_prompt_counts_only_what_has_an_owner() {
        let links = |pairs: &[(&str, Option<&str>)]| {
            pairs
                .iter()
                .map(|(code, owner)| ((*code).to_string(), owner.map(str::to_string)))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            claim_message(&links(&[("solo", Some("alice"))])),
            "Take /solo from alice? They lose it from their list."
        );
        assert!(
            claim_message(&links(&[("a", Some("alice")), ("b", Some("bob"))]))
                .contains("Take 2 links from their owners")
        );
        // A mixed selection owns up to both numbers: the button counts
        // everything it acts on, and only some of that is taken from anybody.
        let mixed = claim_message(&links(&[
            ("a", Some("alice")),
            ("b", None),
            ("c", Some("bob")),
        ]));
        assert!(mixed.contains("Claim 3 links"), "{mixed}");
        assert!(mixed.contains("2 of them are taken"), "{mixed}");
        assert_eq!(claim_message(&[]), "");
    }

    #[test]
    fn the_delete_prompt_counts_what_it_will_remove() {
        assert_eq!(
            delete_message(&["solo".to_string()]),
            "Delete /solo? This cannot be undone."
        );
        assert_eq!(
            delete_message(&["a".to_string(), "b".to_string()]),
            "Delete 2 links? This cannot be undone."
        );
    }
}
