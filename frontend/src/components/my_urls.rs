use std::collections::HashSet;
use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::{UrlEntry, UrlUpsertRequest, validate_code, validate_url};
use wasm_bindgen::JsCast;

use crate::api;
use crate::auth::use_auth;
use crate::components::avatar::{Avatar, usable_url};
use crate::components::confirm::ConfirmDialog;
use crate::toast::use_toasts;
use crate::ui::row_input_class;

const PAGE_SIZE: u64 = 20;
/// Long enough that typing a word is one request, short enough to feel live.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
/// Distance from the bottom at which the next page starts loading, so the rows
/// are usually there before the scrollbar reaches the end.
const LOAD_MORE_MARGIN: f64 = 200.0;
/// Placeholder rows while the first page of a query is on its way.
const GHOST_ROWS: usize = 6;
/// How long a load may take before it is worth showing skeletons. A response
/// that beats this never draws them, so a fast query does not flash.
const GHOST_DELAY: Duration = Duration::from_millis(200);

/// A column the server will sort by. Anything outside `backend::query::SORTABLE`
/// comes back 400, so `Owner` deliberately has no sort control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub field: &'static str,
    pub desc: bool,
}

/// What the table sorts by until told otherwise: most recently touched first,
/// which is what someone opening the page usually wants to see. The server
/// applies the same order when no sort is sent, so cycling a column back to
/// "off" lands here too.
pub const DEFAULT_SORT: Sort = Sort {
    field: "updated_at",
    desc: true,
};

/// Click cycle for a column: ascending, then descending, then back to no
/// explicit sort at all. Three clicks return to where you started, so there is
/// always a way out without hunting for the original column.
///
/// `None` means "whatever the server sorts by", which is why the signal holds
/// an `Option` rather than defaulting to `created_at` here — that would make
/// the third click on *that* column indistinguishable from the second.
fn cycled(current: Option<Sort>, field: &'static str) -> Option<Sort> {
    match current {
        Some(sort) if sort.field == field => {
            if sort.desc {
                None
            } else {
                Some(Sort { field, desc: true })
            }
        }
        _ => Some(Sort { field, desc: false }),
    }
}

/// Percent-encodes a search term so `&`, `#` and friends cannot break out of
/// the query string. Hand-rolled rather than `js_sys::encode_uri_component`,
/// which is a JS binding and panics when the tests run off wasm.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The query string for one page of results. An unset sort is left out so the
/// server applies its own default.
fn list_query(page: u64, search: &str, sort: Option<Sort>) -> String {
    let mut query = format!("limit={PAGE_SIZE}&skip={}", page * PAGE_SIZE);
    if let Some(sort) = sort {
        let direction = if sort.desc { -1 } else { 1 };
        query.push_str(&format!("&sort={},{direction}", sort.field));
    }
    if !search.trim().is_empty() {
        query.push_str(&format!("&q={}", encode(search.trim())));
    }
    query
}

/// `2026-08-01T18:34:37.937Z` reads better as `2026-08-01 18:34`. Anything that
/// is not the expected shape is shown as-is rather than mangled.
fn short_datetime(raw: Option<&String>) -> String {
    let Some(value) = raw else {
        return "—".into();
    };
    match value.split_once('T') {
        Some((date, time)) if time.len() >= 5 => format!("{date} {}", &time[..5]),
        _ => value.clone(),
    }
}

/// True only when there is something to select and all of it is selected — an
/// empty table must not show a ticked "select all".
fn all_selected(codes: &[String], selected: &HashSet<String>) -> bool {
    !codes.is_empty() && codes.iter().all(|code| selected.contains(code))
}

/// What the delete dialog says, for one row or for a whole selection.
fn delete_message(codes: &[String]) -> String {
    match codes {
        [] => String::new(),
        [one] => format!("Delete /{one}? This cannot be undone."),
        many => format!("Delete {} links? This cannot be undone.", many.len()),
    }
}

/// A column header that sorts. Owns the toggle rather than taking a callback,
/// since the only thing it does is rewrite the shared `Sort`.
#[component]
fn SortHeader(
    field: &'static str,
    label: &'static str,
    sort: RwSignal<Option<Sort>>,
    /// Fixed width for the column, so it does not jump when the cell swaps
    /// between text, an edit input and a loading skeleton.
    #[prop(default = "")]
    width: &'static str,
) -> impl IntoView {
    let icon = move || match sort.get() {
        Some(current) if current.field == field && current.desc => {
            "icon-[tabler--sort-descending] text-primary"
        }
        Some(current) if current.field == field => "icon-[tabler--sort-ascending] text-primary",
        _ => "icon-[tabler--arrows-sort] opacity-30",
    };

    view! {
        <th class=format!("p-0 {width}")>
            <button
                class="flex overflow-hidden gap-1 items-center py-3 px-3 w-full text-xs font-semibold tracking-wide uppercase hover:bg-base-200"
                on:click=move |_| sort.update(|s| *s = cycled(*s, field))
            >
                <span class="truncate">{label}</span>
                <span class=move || format!("{} size-4", icon())></span>
            </button>
        </th>
    }
}

/// One placeholder row, shown while the first page of a new query loads.
#[component]
fn GhostRow(columns: usize) -> impl IntoView {
    view! {
        <tr>
            {(0..columns)
                .map(|_| {
                    view! {
                        <td>
                            <span class="block w-full h-4 rounded animate-pulse bg-base-content/10"></span>
                        </td>
                    }
                })
                .collect_view()}
        </tr>
    }
}

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
    // Which field failed local validation, if any. Only a red border: the rule
    // it broke is the same one the placeholder implies, and a message per row
    // would push the table around.
    let (invalid_field, set_invalid_field) = signal(Option::<&'static str>::None);
    let (saving, set_saving) = signal(false);
    let pending_delete = RwSignal::new(Vec::<String>::new());
    let confirm_open = RwSignal::new(false);
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
        let query = list_query(index, &debounced.get_untracked(), sort.get_untracked());

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
        set_timeout(
            move || {
                // Still the current request, and still waiting.
                if generation.get_untracked() == mine && loading.get_untracked() {
                    set_slow.set(true);
                }
            },
            GHOST_DELAY,
        );
        spawn_local(async move {
            let result = api::list_urls(&query, signal.as_ref()).await;
            // A newer request started while this one was out — including the
            // one that aborted it, whose error is not worth showing.
            if generation.get_untracked() != mine {
                return;
            }
            match result {
                Ok(response) => {
                    set_total.set(response.total);
                    if append {
                        set_items.update(|rows| rows.extend(response.items));
                    } else {
                        set_items.set(response.items);
                    }
                    set_page.set(index);
                }
                Err(err) => toasts.error(err.message),
            }
            set_loading.set(false);
            set_slow.set(false);
            set_loaded_once.set(true);
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
                if keystroke.get_untracked() == mine {
                    set_debounced.set(search.get_untracked());
                }
            },
            SEARCH_DEBOUNCE,
        );
    };

    // Deletes each code in turn and drops its row. Refetching instead would
    // throw away every page scrolled so far.
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
        set_invalid_field.set(None);
        editing.set(Some(entry.code));
    };

    let cancel_edit = move || {
        editing.set(None);
        set_invalid_field.set(None);
    };

    let save_edit = move |original: String| {
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
        set_invalid_field.set(None);
        set_saving.set(true);
        spawn_local(async move {
            let request = UrlUpsertRequest {
                code,
                url,
                expires_at: None,
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
                Err(err) => toasts.error(err.message),
            }
            set_saving.set(false);
        });
    };

    // Enter commits the row, Escape abandons it — the two keys a keyboard user
    // reaches for once a table cell has turned into an input.
    let edit_keys = move |ev: &leptos::ev::KeyboardEvent, original: &str| match ev.key().as_str() {
        "Enter" => save_edit(original.to_string()),
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
    // Checkbox, code, destination, hits, last hit, created, updated, actions —
    // plus the admin-only owner column.
    let column_count = move || if auth.is_admin() { 9 } else { 8 };

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
                        <Show when=has_selection>
                            // Plain `btn`: `.btn` and `.input` share the same
                            // --size, so this lines up with the search box.
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
                        <input
                            class="w-64 max-w-full input"
                            placeholder="Search code or url"
                            aria-label="Search links"
                            prop:value=search
                            on:input=on_search
                        />
                    </div>
                </div>

                // Fills the viewport below the header and scrolls internally, so
                // the page itself never grows and the next page loads as the
                // bottom comes into view.
                <div
                    class="overflow-auto rounded-lg border h-[calc(100vh-16rem)] border-base-content/10"
                    on:scroll=on_scroll
                >
                    // The header carries its own background, which separates it
                    // from the rows — so the first row needs no rule above it.
                    // FlyonUI already leaves the last row without one below.
                    <table class="table table-fixed min-w-[68rem] [&_thead_tr]:border-b-0 [&_td]:px-3">
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
                                    <th class="px-3 w-28">"Owner"</th>
                                </Show>
                                <SortHeader field="hits" label="Hits" sort=sort width="w-24" />
                                <SortHeader
                                    field="last_hit_at"
                                    label="Last hit"
                                    sort=sort
                                    width="w-36"
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
                                <th class="px-3 w-24"></th>
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

                            // Keyed on content, not just the code: the cells
                            // are captured values, so a row whose code stayed
                            // the same would keep showing its old destination
                            // after an edit. `updated_at` moves on every write.
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
                                    let row_code = entry.code.clone();
                                    let select_code = entry.code.clone();
                                    let check_code = entry.code.clone();
                                    let delete_code = entry.code.clone();
                                    let save_code = entry.code.clone();
                                    let keys_code = entry.code.clone();
                                    let keys_url = entry.code.clone();
                                    let code = entry.code.clone();
                                    let for_edit = entry.clone();
                                    let url = entry.url.clone();
                                    let href = entry.url.clone();
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
                                    let hits = entry.hits;
                                    let last = short_datetime(entry.last_hit_at.as_ref());
                                    let created = short_datetime(entry.created_at.as_ref());
                                    let updated = short_datetime(entry.updated_at.as_ref());
                                    let row_code = StoredValue::new(row_code);
                                    let is_editing = move || {
                                        row_code
                                            .with_value(|code| {
                                                editing.get().as_deref() == Some(code.as_str())
                                            })
                                    };
                                    // Stored rather than captured by value so
                                    // the closure stays `Copy` — both the cell
                                    // columns and the action column ask.
                                    // Every cell is read out of `entry` first:
                                    // `Show`'s children is an `Fn` closure, so
                                    // a conditional column would otherwise move
                                    // the entry away from the ones after it.
                                    view! {
                                        <tr>
                                            <td>
                                                <input
                                                    type="checkbox"
                                                    class="checkbox checkbox-sm"
                                                    aria-label=format!("Select {select_code}")
                                                    prop:checked=move || {
                                                        selected.get().contains(&check_code)
                                                    }
                                                    on:change=move |_| {
                                                        let code = select_code.clone();
                                                        selected
                                                            .update(|set| {
                                                                if !set.remove(&code) {
                                                                    set.insert(code);
                                                                }
                                                            });
                                                    }
                                                />
                                            </td>

                                            <Show
                                                when=is_editing
                                                fallback={
                                                    let code = code.clone();
                                                    let url = url.clone();
                                                    let href = href.clone();
                                                    move || {
                                                        view! {
                                                            <td class="font-mono">{code.clone()}</td>
                                                            <td class="truncate">
                                                                <a
                                                                    href=href.clone()
                                                                    target="_blank"
                                                                    rel="noreferrer"
                                                                    class="link"
                                                                >
                                                                    {url.clone()}
                                                                </a>
                                                            </td>
                                                        }
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
                                                        on:keydown={
                                                            let original = keys_code.clone();
                                                            move |ev| edit_keys(&ev, &original)
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
                                                        on:keydown={
                                                            let original = keys_url.clone();
                                                            move |ev| edit_keys(&ev, &original)
                                                        }
                                                    />
                                                </td>
                                            </Show>

                                            <Show when=move || auth.is_admin()>
                                                <td>
                                                    {if has_owner {
                                                        let name = owner_name.clone();
                                                        view! {
                                                            <span class="flex gap-2 items-center">
                                                                <Avatar
                                                                    url=owner_avatar.clone()
                                                                    name=name.clone()
                                                                    size="size-6"
                                                                />
                                                                <span class="opacity-70">{name}</span>
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
                                            <td class="whitespace-nowrap opacity-70">{created}</td>
                                            <td class="whitespace-nowrap opacity-70">{updated}</td>
                                            <td class="whitespace-nowrap">
                                                <span class="flex gap-1 justify-end">
                                                    <Show
                                                        when=is_editing
                                                        fallback={
                                                            let for_edit = for_edit.clone();
                                                            let delete_code = delete_code.clone();
                                                            move || {
                                                                let for_edit = for_edit.clone();
                                                                let delete_code = delete_code.clone();
                                                                view! {
                                                                    <button
                                                                        class="btn btn-text btn-sm btn-square"
                                                                        aria-label="Edit link"
                                                                        title="Edit"
                                                                        on:click=move |_| begin_edit(for_edit.clone())
                                                                    >
                                                                        <span class="icon-[tabler--pencil] size-4"></span>
                                                                    </button>
                                                                    <button
                                                                        class="btn btn-text btn-sm btn-square text-error"
                                                                        aria-label="Delete link"
                                                                        title="Delete"
                                                                        on:click=move |_| { ask_delete(vec![delete_code.clone()]) }
                                                                    >
                                                                        <span class="icon-[tabler--trash] size-4"></span>
                                                                    </button>
                                                                }
                                                            }
                                                        }
                                                    >
                                                        <button
                                                            class="btn btn-primary btn-sm btn-square"
                                                            aria-label="Save changes"
                                                            title="Save"
                                                            disabled=move || saving.get()
                                                            on:click={
                                                                let save_code = save_code.clone();
                                                                move |_| save_edit(save_code.clone())
                                                            }
                                                        >
                                                            <span class="icon-[tabler--check] size-4"></span>
                                                        </button>
                                                        <button
                                                            class="btn btn-text btn-sm btn-square"
                                                            aria-label="Cancel editing"
                                                            title="Cancel"
                                                            on:click=move |_| cancel_edit()
                                                        >
                                                            <span class="icon-[tabler--x] size-4"></span>
                                                        </button>
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
                open=confirm_open
                title="Delete links"
                message=Signal::derive(move || delete_message(&pending_delete.get()))
                confirm_label="Delete"
                on_confirm=delete_confirmed
            />
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BY_CODE: Sort = Sort {
        field: "code",
        desc: false,
    };

    #[test]
    fn the_query_carries_paging_sort_and_only_a_real_search() {
        // No sort at all: the server picks, rather than the UI guessing.
        assert_eq!(list_query(0, "", None), "limit=20&skip=0");
        assert_eq!(list_query(2, "   ", None), "limit=20&skip=40");
        assert_eq!(
            list_query(1, "abc", Some(BY_CODE)),
            "limit=20&skip=20&sort=code,1&q=abc"
        );
        assert_eq!(
            list_query(
                0,
                "",
                Some(Sort {
                    field: "hits",
                    desc: true
                })
            ),
            "limit=20&skip=0&sort=hits,-1"
        );
    }

    /// An unencoded `&` or `#` would end the parameter and silently drop the
    /// rest of the term.
    #[test]
    fn the_search_term_is_percent_encoded() {
        assert!(list_query(0, "a&b", None).ends_with("&q=a%26b"));
        assert!(list_query(0, "a b", None).ends_with("&q=a%20b"));
    }

    /// Three clicks on one column return to the starting state.
    #[test]
    fn a_column_cycles_ascending_descending_then_off() {
        let first = cycled(None, "code");
        assert_eq!(first, Some(BY_CODE));
        let second = cycled(first, "code");
        assert_eq!(
            second,
            Some(Sort {
                field: "code",
                desc: true
            })
        );
        assert_eq!(cycled(second, "code"), None, "the third click clears it");
    }

    #[test]
    fn a_different_column_starts_ascending_rather_than_inheriting() {
        let descending_code = Some(Sort {
            field: "code",
            desc: true,
        });
        assert_eq!(
            cycled(descending_code, "hits"),
            Some(Sort {
                field: "hits",
                desc: false
            })
        );
    }

    #[test]
    fn timestamps_are_shortened_to_the_minute() {
        assert_eq!(
            short_datetime(Some(&"2026-08-01T18:34:37.937Z".to_string())),
            "2026-08-01 18:34"
        );
        assert_eq!(short_datetime(None), "—");
        // Anything unexpected is shown rather than sliced into nonsense.
        assert_eq!(short_datetime(Some(&"whenever".to_string())), "whenever");
    }

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
