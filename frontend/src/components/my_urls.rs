use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::{UrlBulkAction, UrlEntry, UrlUpsertRequest, validate_code, validate_url};
use wasm_bindgen::JsCast;

use crate::api;
use crate::auth::use_auth;
use crate::clipboard::{copy, origin, short_link};
use crate::components::avatar::{Avatar, usable_url};
use crate::components::confirm::ConfirmDialog;
use crate::components::conflict::other_owner;
use crate::components::edit_link::EditLinkDialog;
use crate::components::tooltip::Tooltip;
use crate::components::url_filters::FilterDialog;
use crate::datetime::{
    from_display, from_rfc3339, is_future, local_offset_minutes, to_display, to_rfc3339,
};
use crate::filters::Filters;
use crate::list::{
    GHOST_DELAY, GHOST_ROWS, GhostRow, LOAD_MORE_MARGIN, SEARCH_DEBOUNCE, Sort, SortHeader,
    URL_SORT_FIELDS, all_selected, list_params, parse_sort, short_datetime, sort_param,
};
use crate::router::{replace_query, use_hash_query};
use crate::toast::use_toasts;

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

/// What a refused selection says. The count matters: one link out of forty is
/// a different thing to untick than thirty-nine.
fn refusal_message(message: &str, rejected: &[shared::RejectedId]) -> String {
    let Some(first) = rejected.first() else {
        return message.to_string();
    };
    let links = match rejected.len() {
        1 => "1 link was".to_string(),
        n => format!("{n} links were"),
    };
    format!("{message}: {links} refused — {}", first.message)
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
    let filters = RwSignal::new(Filters::default());
    let filter_open = RwSignal::new(false);
    let selected = RwSignal::new(HashSet::<String>::new());
    // Keyed by the code editing began with: a rename changes it.
    let editing = RwSignal::new(Option::<String>::None);
    let edit_open = RwSignal::new(false);
    let draft_code = RwSignal::new(String::new());
    let draft_url = RwSignal::new(String::new());
    let draft_expiry = RwSignal::new(String::new());
    // The link a save collided with, and whether landing on it destroys it.
    let conflict = RwSignal::new(None::<(UrlEntry, bool)>);
    let reset_hits = RwSignal::new(false);
    let claim_owner = RwSignal::new(false);
    // A red border only: the placeholder already implies the rule.
    let invalid_field = RwSignal::new(Option::<&'static str>::None);
    let (saving, set_saving) = signal(false);
    let pending_delete = RwSignal::new(Vec::<String>::new());
    let confirm_open = RwSignal::new(false);
    // Only a takeover needs confirming; an unowned link is nobody's to defend.
    let pending_claim = RwSignal::new(Vec::<UrlEntry>::new());
    let claim_open = RwSignal::new(false);
    // Local, because a JS object is neither Send nor Sync.
    let inflight = StoredValue::new_local(Option::<web_sys::AbortController>::None);
    // Guards against a cancelled or overtaken response writing stale rows.
    let (generation, set_generation) = signal(0u32);
    // Raised only once a load has been slow enough to be worth showing.
    let (slow, set_slow) = signal(false);

    // Untracked throughout, so callers decide when it runs.
    let fetch = move |index: u64, append: bool| {
        if auth.user.get_untracked().is_none() {
            return;
        }
        let mut params = list_params(index, &debounced.get_untracked(), sort.get_untracked());
        params.extend(filters.get_untracked().to_params(local_offset_minutes()));

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
        // `try_` throughout: writing a disposed signal is fatal in wasm.
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
            // A newer request started while this was out, or the page is gone.
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

    // A new query invalidates every page accumulated, and any open editor.
    Effect::new(move |_| {
        auth.user.get();
        debounced.get();
        sort.get();
        filters.get();
        editing.set(None);
        // A tick against a row no longer listed would delete it unseen.
        selected.set(HashSet::new());
        set_items.set(Vec::new());
        fetch(0, false);
    });

    let more_to_load = move || (items.get().len() as u64) < total.get();
    // Only for a fresh query, and only once `slow` says it is worth it.
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

    // `hydrated`, so the writer below cannot overwrite an incoming query.
    let hash_query = use_hash_query();
    let hydrated = RwSignal::new(false);
    Effect::new(move |_| {
        let pairs = hash_query.get();
        let typed = pairs
            .iter()
            .find(|(key, _)| key == "q")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        let wanted = pairs
            .iter()
            .find(|(key, _)| key == "sort")
            .and_then(|(_, value)| parse_sort(value, URL_SORT_FIELDS))
            .or(Some(DEFAULT_SORT));
        let wanted_filters = Filters::from_pairs(&pairs, local_offset_minutes());
        if search.get_untracked() != typed {
            set_search.set(typed.clone());
            set_debounced.set(typed);
        }
        if sort.get_untracked() != wanted {
            sort.set(wanted);
        }
        if filters.get_untracked() != wanted_filters {
            filters.set(wanted_filters);
        }
        hydrated.set(true);
    });

    Effect::new(move |_| {
        if !hydrated.get() {
            return;
        }
        let mut pairs = Vec::new();
        let typed = debounced.get();
        if !typed.trim().is_empty() {
            pairs.push(("q".to_string(), typed.trim().to_string()));
        }
        if let Some(sort) = sort.get() {
            pairs.push(("sort".to_string(), sort_param(sort)));
        }
        pairs.extend(filters.get().to_params(local_offset_minutes()));
        replace_query(&pairs);
    });

    let active_filters = move || filters.get().active();
    let base_query = Signal::derive(move || {
        let typed = debounced.get();
        if typed.trim().is_empty() {
            Vec::new()
        } else {
            vec![("q".to_string(), typed.trim().to_string())]
        }
    });

    // Clearing skips the debounce: there is nothing more to type.
    let clear_search = move |_| {
        set_search.set(String::new());
        set_keystroke.update(|n| *n += 1);
        set_debounced.set(String::new());
    };

    // One request per hundred, refused whole: one unreachable link stops a chunk.
    let claim_many = move |codes: Vec<String>| {
        spawn_local(async move {
            let outcome = api::bulk_urls(codes, UrlBulkAction::Claim).await;
            let (done, failure) = match outcome {
                Ok(done) => (done, None),
                Err((done, err)) => (done, Some(err)),
            };
            for entry in &done.entries {
                let code = entry.code.clone();
                set_items.update(|rows| {
                    if let Some(row) = rows.iter_mut().find(|row| row.code == code) {
                        *row = entry.clone();
                    }
                });
                selected.update(|set| {
                    set.remove(&code);
                });
            }
            let owner = done
                .entries
                .first()
                .and_then(|entry| entry.owner.as_ref())
                .and_then(|owner| owner.username.clone())
                .unwrap_or_default();
            match (done.affected, failure) {
                (0, None) => {}
                (1, None) => toasts.success(format!("1 link is {owner}'s now")),
                (n, None) => toasts.success(format!("{n} links are {owner}'s now")),
                (n, Some(err)) => {
                    if n > 0 {
                        toasts.success(format!("{n} claimed before it stopped"));
                    }
                    toasts.error(refusal_message(
                        &err.message,
                        err.rejected.as_deref().unwrap_or_default(),
                    ));
                }
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

    // Always asks: even a claim that takes nothing can be refused whole.
    let ask_claim = move |entries: Vec<UrlEntry>| {
        pending_claim.set(entries);
        claim_open.set(true);
    };

    let delete_confirmed = Callback::new(move |()| {
        let codes = pending_delete.get_untracked();
        spawn_local(async move {
            let outcome = api::bulk_urls(codes, UrlBulkAction::Delete).await;
            let (done, failure) = match outcome {
                Ok(done) => (done, None),
                Err((done, err)) => (done, Some(err)),
            };
            // A delete answers with no entries, so the codes asked for are the ones gone.
            if done.affected > 0 {
                let gone: HashSet<String> = pending_delete
                    .get_untracked()
                    .into_iter()
                    .take(done.affected as usize)
                    .collect();
                set_items.update(|rows| rows.retain(|row| !gone.contains(&row.code)));
                set_total.update(|total| *total = total.saturating_sub(done.affected));
                selected.update(|set| set.retain(|code| !gone.contains(code)));
            }
            match (done.affected, failure) {
                (0, None) => {}
                (n, None) => toasts.success(format!("Deleted {n} link(s)")),
                (n, Some(err)) => {
                    if n > 0 {
                        toasts.success(format!("Deleted {n} before it stopped"));
                    }
                    toasts.error(refusal_message(
                        &err.message,
                        err.rejected.as_deref().unwrap_or_default(),
                    ));
                }
            }
        });
    });

    let ask_delete = move |codes: Vec<String>| {
        pending_delete.set(codes);
        confirm_open.set(true);
    };

    let begin_edit = move |entry: UrlEntry| {
        draft_code.set(entry.code.clone());
        draft_url.set(entry.url.clone());
        draft_expiry.set(
            entry
                .expires_at
                .as_deref()
                .and_then(|stamp| from_rfc3339(stamp, local_offset_minutes()))
                .and_then(|local| to_display(&local))
                .unwrap_or_default(),
        );
        invalid_field.set(None);
        conflict.set(None);
        reset_hits.set(false);
        claim_owner.set(false);
        editing.set(Some(entry.code));
        edit_open.set(true);
    };

    let save_edit = move |original: String, overwrite: bool| {
        let code = draft_code.get_untracked().trim().to_string();
        let url = draft_url.get_untracked().trim().to_string();
        // The server's own rules, so a typo costs no round trip.
        if validate_code(&code).is_err() {
            invalid_field.set(Some("code"));
            return;
        }
        if validate_url(&url).is_err() {
            invalid_field.set(Some("url"));
            return;
        }
        // Empty is a removal, so one box both sets and clears a deadline.
        let shown = draft_expiry.get_untracked();
        let expires_at = match shown.trim() {
            "" => String::new(),
            shown => {
                let stamp = from_display(shown)
                    .and_then(|local| to_rfc3339(&local, local_offset_minutes()))
                    .filter(|stamp| is_future(stamp));
                let Some(stamp) = stamp else {
                    invalid_field.set(Some("expires_at"));
                    return;
                };
                stamp
            }
        };
        invalid_field.set(None);
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
                    let mut destroyed = 0usize;
                    set_items.update(|rows| {
                        let Some(index) = rows.iter().position(|row| row.code == original) else {
                            return;
                        };
                        rows[index] = updated;
                        // Landing on another of your links deletes it.
                        let mut seen = 0usize;
                        rows.retain(|row| {
                            let keep = seen == index || row.code != renamed;
                            seen += 1;
                            destroyed += usize::from(!keep);
                            keep
                        });
                    });
                    if destroyed > 0 {
                        set_total.update(|total| *total = total.saturating_sub(destroyed as u64));
                    }
                    // A rename moves the row's identity, and the tick with it.
                    selected.update(|set| {
                        set.remove(&renamed);
                        if set.remove(&original) {
                            set.insert(renamed);
                        }
                    });
                    editing.set(None);
                    edit_open.set(false);
                    invalid_field.set(None);
                    conflict.set(None);
                    toasts.success("Link saved");
                }
                // A code the caller owns is an offer, not a refusal.
                Err(err) => match err.conflict {
                    Some(existing) => {
                        reset_hits.set(false);
                        claim_owner.set(false);
                        conflict.set(Some((*existing, renaming)));
                    }
                    None => toasts.error(err.message),
                },
            }
            set_saving.set(false);
        });
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
    // Out of the view: leptosfmt reads a `>` in an attribute as a closing bracket.
    let has_selection = move || selected_count() > 0;
    let claimable_selection = move || {
        let ticked = selected.get();
        items
            .get()
            .into_iter()
            .filter(|row| ticked.contains(&row.code) && row.claimable)
            .collect::<Vec<_>>()
    };
    let can_claim = move || !claimable_selection().is_empty();
    // Every column, plus the admin-only owner one.
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
                        <button
                            class="relative gap-2 btn btn-text"
                            aria-label="Filter links"
                            on:click=move |_| filter_open.set(true)
                        >
                            <span class="icon-[tabler--filter] size-7"></span>
                            <Show when=move || { active_filters() > 0 }>
                                <span class="absolute top-0 right-0 badge badge-primary badge-xs">
                                    {active_filters}
                                </span>
                            </Show>
                        </button>
                    </div>
                </div>

                <div
                    class="overflow-auto rounded-lg border h-[calc(100vh-16rem)] border-base-content/10"
                    on:scroll=on_scroll
                >
                    <table class="table table-fixed table-pinned min-w-[86rem] [&_thead_tr]:border-b-0 [&_td]:px-3">
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
                                    let claimable = entry.claimable;
                                    let editable = entry.editable;
                                    let taking_over = has_owner;
                                    let hits = entry.hits;
                                    let last = short_datetime(entry.last_hit_at.as_ref());
                                    let expires = short_datetime(entry.expires_at.as_ref());
                                    let created = short_datetime(entry.created_at.as_ref());
                                    let updated = short_datetime(entry.updated_at.as_ref());
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
                                            <td class="whitespace-nowrap opacity-70">{expires}</td>
                                            <td class="whitespace-nowrap opacity-70">{created}</td>
                                            <td class="whitespace-nowrap opacity-70">{updated}</td>
                                            <td class="whitespace-nowrap">
                                                <span class="flex gap-1 justify-end">
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
                                                    <Show when=move || editable>
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

            <FilterDialog
                open=filter_open
                filters=filters
                is_admin=Signal::derive(move || auth.is_admin())
                base=base_query
            />

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

            <EditLinkDialog
                open=edit_open
                original=Signal::derive(move || editing.get().unwrap_or_default())
                code=draft_code
                url=draft_url
                expiry=draft_expiry
                invalid=invalid_field
                saving=Signal::derive(move || saving.get())
                conflict=conflict
                reset_hits=reset_hits
                claim=claim_owner
                other_owner=Signal::derive(move || {
                    conflict
                        .get()
                        .and_then(|(existing, _)| {
                            other_owner(&existing, &auth).and(existing.owner)
                        })
                })
                on_save=Callback::new(move |overwrite: bool| {
                    if let Some(original) = editing.get_untracked() {
                        save_edit(original, overwrite);
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
        // A mixed selection owns up to both numbers.
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
    fn a_refusal_counts_the_links_it_was_about() {
        let refused = |n: usize| {
            (0..n)
                .map(|i| shared::RejectedId {
                    id: format!("c{i}"),
                    code: "forbidden".into(),
                    message: "not yours".into(),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            refusal_message("nothing changed", &refused(1)),
            "nothing changed: 1 link was refused — not yours"
        );
        assert!(refusal_message("nothing changed", &refused(3)).contains("3 links were refused"));
        assert_eq!(refusal_message("it broke", &[]), "it broke");
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
