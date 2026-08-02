//! The admin Users page: the chain as a tree, everyone else paged beneath it.

pub mod row;
pub mod text;
pub mod tree;

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::{AdminOrphans, AdminUserInfo};
use wasm_bindgen::JsCast;

use crate::api;
use crate::auth::use_auth;
use crate::components::confirm::ConfirmDialog;
use crate::list::{
    GHOST_DELAY, GHOST_ROWS, GhostRow, LOAD_MORE_MARGIN, SEARCH_DEBOUNCE, Sort, SortHeader,
    all_selected, list_params,
};
use crate::toast::use_toasts;

use row::{UserRow, row_key};
use text::{GRACE_DAYS, bulk_delete_warning, bulk_demote_warning, delete_warning, demote_warning};
use tree::{
    descendants, has_children, matches, may_manage, orphaned_admins, parents, search_tree,
    tree_order, visible_rows,
};

/// What the confirmation dialog is about. Demoting and deleting both need one,
/// singly and in bulk, and only one dialog can be open — so which it is travels
/// with the rows it would act on.
#[derive(Clone)]
pub enum Pending {
    Demote(Vec<AdminUserInfo>),
    Delete(Vec<AdminUserInfo>),
}

#[component]
pub fn Users() -> impl IntoView {
    let auth = use_auth();
    let toasts = use_toasts();

    let (admins, set_admins) = signal(Vec::<AdminUserInfo>::new());
    let (items, set_items) = signal(Vec::<AdminUserInfo>::new());
    let (total, set_total) = signal(0u64);
    let (page, set_page) = signal(0u64);
    let (loading, set_loading) = signal(false);
    let (slow, set_slow) = signal(false);
    let (search, set_search) = signal(String::new());
    let (debounced, set_debounced) = signal(String::new());
    let (keystroke, set_keystroke) = signal(0u32);
    let sort: RwSignal<Option<Sort>> = RwSignal::new(None);

    // Which branches are folded, and the two halves of a drag in progress.
    // They live here rather than in a row because both ends of a drag, and
    // both a parent and its branch, are different rows.
    let collapsed: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let dragging: RwSignal<Option<AdminUserInfo>> = RwSignal::new(None);
    let drop_target: RwSignal<Option<String>> = RwSignal::new(None);
    let scrolled: RwSignal<u32> = RwSignal::new(0);

    let confirm_open = RwSignal::new(false);
    // What the dialog is about. One dialog rather than four, because only one
    // can be open, and the rows travel with it so the wording can count them.
    let pending: RwSignal<Option<Pending>> = RwSignal::new(None);
    // Rows ticked for a bulk action, by id. Ids rather than rows because a
    // reload replaces every row but keeps the ids.
    let selected: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    // Measured rather than guessed: the group headings stick directly under the
    // table header, and a hard-coded offset drifts as soon as the header's
    // height does — leaving a gap above them, or hiding a row behind them.
    let head: NodeRef<leptos::html::Thead> = NodeRef::new();
    let (head_height, set_head_height) = signal(40);
    Effect::new(move |_| {
        if let Some(node) = head.get() {
            set_head_height.set(node.offset_height());
        }
    });
    let under_head = move || format!("top:{}px", head_height.get());

    let fetch = move |next_page: u64, append: bool| {
        let params = list_params(next_page, &debounced.get_untracked(), sort.get_untracked());
        set_loading.set(true);
        set_slow.set(false);
        // `try_` throughout: a timer or a request can outlive the page that
        // started it, and writing a signal whose owner has been disposed panics
        // — which in wasm is fatal to the whole app, not just to this page.
        set_timeout(
            move || {
                if loading.try_get_untracked() == Some(true) {
                    set_slow.try_set(true);
                }
            },
            GHOST_DELAY,
        );
        spawn_local(async move {
            match api::admin_users(params).await {
                Ok(response) => {
                    set_admins.try_set(tree_order(&response.admins));
                    set_total.try_set(response.total);
                    if append {
                        set_items.try_update(|rows| rows.extend(response.items));
                    } else {
                        set_items.try_set(response.items);
                    }
                    set_page.try_set(next_page);
                }
                Err(err) => toasts.error(err.message),
            }
            set_loading.try_set(false);
        });
    };

    // A different sort or search invalidates every page accumulated so far.
    //
    // Nothing is asked for until the session has resolved: `auth.user` is
    // `None` both while the first `/api/auth/me` is in flight and when signed
    // out, so firing on it would send one request against no session and a
    // second when it landed. Non-admins are not asked at all — the answer is a
    // 403 and the page shows its own refusal instead.
    Effect::new(move |_| {
        debounced.get();
        sort.get();
        if !auth.loaded.get() || !auth.is_admin() {
            return;
        }
        // Whatever was ticked belongs to the list that is about to be replaced.
        selected.set(HashSet::new());
        set_items.set(Vec::new());
        fetch(0, false);
    });

    let on_search = move |ev| {
        set_search.set(event_target_value(&ev));
        set_keystroke.update(|n| *n += 1);
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

    let clear_search = move |_| {
        set_search.set(String::new());
        set_keystroke.update(|n| *n += 1);
        set_debounced.set(String::new());
    };

    let more_to_load = move || (items.get().len() as u64) < total.get();
    let showing_ghosts = move || slow.get() && loading.get() && items.get().is_empty();

    let on_scroll = move |ev: leptos::ev::Event| {
        scrolled.update(|n| *n = n.wrapping_add(1));
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

    // Re-reads the whole list. A change to the tree can move rows between the
    // two groups and change everyone's depth, so patching in place would be
    // guesswork.
    let reload = move || {
        // Whatever was ticked has just been acted on, one way or another —
        // through the row's own switch as much as through the bulk bar. Leaving
        // the ticks behind would arm the next bulk action with rows the admin
        // thought they were done with.
        selected.try_set(HashSet::new());
        fetch(0, false);
    };

    let apply = move |id: String,
                      is_admin: bool,
                      parent: Option<String>,
                      orphans: AdminOrphans,
                      done: &'static str| {
        spawn_local(async move {
            match api::set_user_admin(&id, is_admin, parent, orphans).await {
                Ok(response) => {
                    let note = match (response.demoted, response.reparented) {
                        (_, moved) if moved > 0 => {
                            format!("{done} ({moved} kept their admin flag)")
                        }
                        (count, _) if count > 1 => format!("{done} ({count} in total)"),
                        _ => done.to_string(),
                    };
                    toasts.success(note);
                    reload();
                }
                Err(err) => toasts.error(err.message),
            }
        });
    };

    let on_promote = Callback::new(move |user: AdminUserInfo| {
        apply(user.id, true, None, AdminOrphans::Demote, "Promoted");
    });
    let on_move = Callback::new(move |(user, parent): (AdminUserInfo, String)| {
        apply(user.id, true, Some(parent), AdminOrphans::Demote, "Moved");
    });
    // What a delete should do with the admins the account promoted. Reset every
    // time the dialog opens: the safe answer is the one that never hands anyone
    // a standing they were not given directly.
    let orphans = RwSignal::new(AdminOrphans::Demote);
    let ask = move |what: Pending| {
        orphans.set(AdminOrphans::Demote);
        pending.set(Some(what));
        confirm_open.set(true);
    };
    let on_demote = Callback::new(move |user: AdminUserInfo| ask(Pending::Demote(vec![user])));
    let on_delete = Callback::new(move |user: AdminUserInfo| ask(Pending::Delete(vec![user])));

    // One request per row, in sequence rather than at once: each one can move
    // the tree under the next, and the server answers a single account at a
    // time. Demotes run deepest first, so a row is dealt with before the
    // cascade from its parent reaches it and turns its own call into an error.
    let run_each = move |mut rows: Vec<AdminUserInfo>, promoting: Option<bool>| {
        rows.sort_by_key(|row| std::cmp::Reverse(row.admin_level.unwrap_or(0)));
        let choice = orphans.get_untracked();
        spawn_local(async move {
            let (mut done, mut failed) = (0usize, 0usize);
            let mut last: Option<String> = None;
            for row in rows {
                let result = match promoting {
                    Some(is_admin) => api::set_user_admin(&row.id, is_admin, None, choice)
                        .await
                        .map(|_| ()),
                    None => api::delete_user(&row.id, choice).await.map(|_| ()),
                };
                match result {
                    Ok(()) => done += 1,
                    Err(err) => {
                        failed += 1;
                        last = Some(err.message);
                    }
                }
            }
            let word = match promoting {
                Some(true) => "promoted",
                Some(false) => "demoted",
                None => "deleted",
            };
            if done > 0 {
                toasts.success(format!("{done} {word}"));
            }
            if let Some(message) = last {
                toasts.error(format!("{failed} failed: {message}"));
            }
            reload();
        });
    };

    let confirmed = Callback::new(move |()| {
        let Some(what) = pending.get_untracked() else {
            return;
        };
        let choice = orphans.get_untracked();
        match what {
            Pending::Demote(rows) if rows.len() == 1 => {
                apply(rows[0].id.clone(), false, None, choice, "Admin removed");
            }
            Pending::Demote(rows) => run_each(rows, Some(false)),
            Pending::Delete(rows) if rows.len() == 1 => {
                let user = rows[0].clone();
                let choice = orphans.get_untracked();
                spawn_local(async move {
                    match api::delete_user(&user.id, choice).await {
                        Ok(response) => {
                            toasts.success(match response.orphaned {
                                0 => format!("Deleted {}", user.username),
                                n => format!(
                                    "Deleted {} — {n} link(s) unowned, expiring in {} days",
                                    user.username, response.grace_days
                                ),
                            });
                            reload();
                        }
                        Err(err) => toasts.error(err.message),
                    }
                });
            }
            Pending::Delete(rows) => run_each(rows, None),
        }
    });

    // What the delete dialog needs to know about the branch below what is about
    // to go: whether there is a choice to make at all, and how to word it.
    let orphans_of_pending = move || match pending.get() {
        Some(Pending::Delete(rows) | Pending::Demote(rows)) => {
            orphaned_admins(&rows, &admins.get())
        }
        None => Vec::new(),
    };
    let has_orphans = move || !orphans_of_pending().is_empty();
    let orphan_summary = move || match orphans_of_pending().len() {
        1 => "1 admin was promoted by this account.".to_string(),
        n => format!("{n} admins were promoted by these accounts."),
    };
    // Named after where they would land, since that is the whole difference —
    // and a root's children have nowhere above to go, so they become roots.
    let reparent_label = move || {
        let rows = orphans_of_pending();
        let parents_left: HashSet<Option<String>> = match pending.get() {
            Some(Pending::Delete(going) | Pending::Demote(going)) => {
                going.iter().map(|row| row.promoted_by.clone()).collect()
            }
            None => HashSet::new(),
        };
        let tree = admins.get();
        let named = |id: &Option<String>| {
            id.as_ref().and_then(|id| {
                tree.iter()
                    .find(|row| &row.id == id)
                    .map(|row| row.username.clone())
            })
        };
        match (rows.len(), parents_left.len()) {
            (_, 1) => match named(parents_left.iter().next().unwrap_or(&None)) {
                Some(name) => format!("Keep them as admins, under {name}"),
                None => "Keep them as admins, answering to nobody".to_string(),
            },
            _ => "Keep them as admins, under whoever is above each account".to_string(),
        }
    };

    // The dialog's wording depends on which action opened it, and the cascade
    // count comes from the tree already loaded.
    let dialog_title = move || match pending.get() {
        Some(Pending::Delete(rows)) if rows.len() == 1 => "Delete account".to_string(),
        Some(Pending::Delete(rows)) => format!("Delete {} accounts", rows.len()),
        _ => "Remove admin".to_string(),
    };
    let dialog_message = move || match pending.get() {
        None => String::new(),
        Some(Pending::Delete(rows)) if rows.len() == 1 => delete_warning(&rows[0], GRACE_DAYS),
        Some(Pending::Delete(rows)) => bulk_delete_warning(&rows, GRACE_DAYS),
        // The cascade count is left out when the radios are up: taking the
        // branch down is only one of the two answers on offer there, so
        // stating it as what will happen would contradict the other.
        Some(Pending::Demote(rows)) if rows.len() == 1 => {
            let below = if has_orphans() {
                0
            } else {
                descendants(&rows[0].id, &admins.get()).len()
            };
            demote_warning(&rows[0], below)
        }
        Some(Pending::Demote(rows)) if has_orphans() => bulk_demote_warning(&rows, 0),
        Some(Pending::Demote(rows)) => {
            // Only the ones the cascade would catch that are not ticked
            // already: counting the rest would double-count the selection.
            let ticked: HashSet<&str> = rows.iter().map(|row| row.id.as_str()).collect();
            let tree = admins.get();
            let below: HashSet<String> = rows
                .iter()
                .flat_map(|row| descendants(&row.id, &tree))
                .filter(|row| !ticked.contains(row.id.as_str()))
                .map(|row| row.id.clone())
                .collect();
            bulk_demote_warning(&rows, below.len())
        }
    };

    let parent_of = move || parents(&admins.get());
    let needle = move || debounced.get().trim().to_lowercase();
    let is_match = move |user: &AdminUserInfo| matches(user, &needle());

    // A new search unfolds whatever it had to reach: a hit hidden inside a
    // branch someone folded earlier would look like the search missed it. Only
    // on the way in, so the chevrons still work while the search is up.
    Effect::new(move |_| {
        let needle = needle();
        if needle.is_empty() {
            return;
        }
        // Untracked: this is about the search changing, not the list reloading.
        // Tracking it would re-open branches after every promote or delete.
        let shown: HashSet<String> = search_tree(&admins.get_untracked(), &needle)
            .into_iter()
            .map(|row| row.id)
            .collect();
        collapsed.update(|folded| folded.retain(|id| !shown.contains(id)));
    });

    // The tree the search left behind, and then that folded. Two steps, because
    // the chevron is decided by what a row still has under it in the *searched*
    // tree — asking the folded one would drop the chevron the moment it was
    // used, and there would be no way to unfold again.
    let found_rows = Memo::new(move |_| search_tree(&admins.get(), &needle()));
    // Read against the searched tree rather than the folded one: asking the
    // folded list would drop the chevron the moment it was used, leaving no way
    // to unfold again.
    let branching = move |user: &AdminUserInfo| {
        user.is_admin && found_rows.with_untracked(|rows| has_children(&user.id, rows))
    };
    let admin_rows =
        Memo::new(move |_| found_rows.with(|rows| visible_rows(rows, &collapsed.get())));
    // What the group heading counts: the whole chain normally, the hits alone
    // while searching — the ancestors dragged along by `search_tree` are there
    // to hold the shape, not because they answered the search.
    let admin_count = move || {
        let needle = needle();
        admins.with(|rows| {
            if needle.is_empty() {
                rows.len()
            } else {
                rows.iter().filter(|row| matches(row, &needle)).count()
            }
        })
    };

    // Only rows this admin could actually act on can be ticked: the server
    // would refuse the rest, and a checkbox that leads to a 403 is a trap.
    let selectable = move || {
        let Some(me) = auth.user.get() else {
            return Vec::new();
        };
        let parent_of = parents(&admins.get());
        admin_rows
            .get()
            .into_iter()
            .chain(items.get())
            .filter(|row| may_manage(&me, row, &parent_of))
            .map(|row| row.id)
            .collect::<Vec<_>>()
    };
    let toggle_all = move |_| {
        let ids = selectable();
        let clear = all_selected(&ids, &selected.get_untracked());
        selected.update(|set| {
            for id in ids {
                if clear {
                    set.remove(&id);
                } else {
                    set.insert(id);
                }
            }
        });
    };
    // The ticked rows themselves, from both groups. A row that has since left
    // the list simply drops out.
    let chosen = move || {
        let ticked = selected.get();
        admin_rows
            .get()
            .into_iter()
            .chain(items.get())
            .filter(|row| ticked.contains(&row.id))
            .collect::<Vec<_>>()
    };
    let chosen_admins = move || {
        chosen()
            .into_iter()
            .filter(|row| row.is_admin)
            .collect::<Vec<_>>()
    };
    let chosen_others = move || {
        chosen()
            .into_iter()
            .filter(|row| !row.is_admin)
            .collect::<Vec<_>>()
    };
    // Kept out of the view: leptosfmt reads the `>` of a comparison inside an
    // attribute as the element's closing bracket and mangles the markup.
    let has_selection = move || !selected.get().is_empty();
    let can_promote = move || !chosen_others().is_empty();
    let can_demote = move || !chosen_admins().is_empty();

    view! {
        <Show
            when=move || auth.is_admin()
            fallback=|| {
                view! {
                    <p class="py-16 text-center text-base-content/60">"This page is for admins."</p>
                }
            }
        >
            <div class="flex flex-col gap-4 motion-preset-fade motion-duration-500">
                <div class="flex flex-wrap gap-3 justify-between items-center">
                    <h2 class="text-3xl font-bold">"Users"</h2>
                    <Show when=has_selection>
                        <div class="flex flex-wrap gap-2 items-center">
                            <span class="text-sm text-base-content/60">
                                {move || format!("{} selected", selected.get().len())}
                            </span>
                            <Show when=can_promote>
                                <button
                                    class="gap-2 btn btn-primary btn-sm"
                                    on:click=move |_| run_each(chosen_others(), Some(true))
                                >
                                    <span class="icon-[tabler--shield-plus] size-4"></span>
                                    {move || format!("Make admin ({})", chosen_others().len())}
                                </button>
                            </Show>
                            <Show when=can_demote>
                                <button
                                    class="gap-2 btn btn-warning btn-sm"
                                    on:click=move |_| ask(Pending::Demote(chosen_admins()))
                                >
                                    <span class="icon-[tabler--shield-minus] size-4"></span>
                                    {move || format!("Remove admin ({})", chosen_admins().len())}
                                </button>
                            </Show>
                            <button
                                class="gap-2 btn btn-error btn-sm"
                                on:click=move |_| ask(Pending::Delete(chosen()))
                            >
                                <span class="icon-[tabler--trash] size-4"></span>
                                {move || format!("Delete ({})", chosen().len())}
                            </button>
                        </div>
                    </Show>
                </div>

                <label class="flex gap-2 items-center input">
                    <span class="opacity-60 icon-[tabler--search] size-4"></span>
                    // Deliberately not `type="search"`: browsers draw their own
                    // clear button inside one, next to the app's.
                    <input
                        type="text"
                        class="grow"
                        placeholder="Search name or email…"
                        prop:value=move || search.get()
                        on:input=on_search
                    />
                    <Show when=move || !search.get().is_empty()>
                        <button
                            class="btn btn-text btn-xs btn-square"
                            aria-label="Clear search"
                            on:click=clear_search
                        >
                            <span class="icon-[tabler--x] size-4"></span>
                        </button>
                    </Show>
                </label>

                // One table, two groups: the admin chain first, indented by
                // depth, then everyone else. Grouping rather than two tables
                // keeps the columns aligned down the whole page.
                <div
                    class="overflow-auto rounded-lg border max-h-[70vh] border-base-content/10"
                    on:scroll=on_scroll
                >
                    <table class="table table-fixed min-w-[60rem] [&_thead_tr]:border-b-0 [&_td]:px-3">
                        <thead node_ref=head class="sticky top-0 z-10 bg-base-200">
                            // `border-0` on the row itself: the group headings
                            // below carry the same background, so a rule here
                            // would only cut one band in two.
                            <tr class="border-0">
                                <th class="px-3 w-10">
                                    <input
                                        type="checkbox"
                                        class="checkbox checkbox-sm"
                                        aria-label="Select all"
                                        prop:checked=move || {
                                            all_selected(&selectable(), &selected.get())
                                        }
                                        on:change=toggle_all
                                    />
                                </th>
                                <SortHeader field="username" label="User" sort=sort />
                                <th class="px-3 w-56">"Signs in with"</th>
                                <SortHeader
                                    field="url_count"
                                    label="Links"
                                    sort=sort
                                    width="w-24"
                                />
                                <SortHeader
                                    field="created_at"
                                    label="Created"
                                    sort=sort
                                    width="w-40"
                                />
                                <th class="px-3 w-32">"Admin"</th>
                                <th class="px-3 w-24"></th>
                            </tr>
                        </thead>
                        <tbody>
                            <Show when=move || !admin_rows.with(Vec::is_empty)>
                                <tr class="border-0">
                                    // No rules of its own: the header above it
                                    // and the rows below both draw their own,
                                    // and its background is separation enough.
                                    <td
                                        colspan="7"
                                        style=under_head
                                        class="sticky py-2 px-3 text-xs font-semibold uppercase z-[9] bg-base-200 text-base-content/60"
                                    >
                                        {move || format!("Admin · {}", admin_count())}
                                    </td>
                                </tr>
                            </Show>

                            <For
                                each=move || admin_rows.get()
                                key=move |user: &AdminUserInfo| {
                                    row_key(user, branching(user), is_match(user))
                                }
                                let:user
                            >
                                <UserRow
                                    matched=is_match(&user)
                                    branching=branching(&user)
                                    selected=selected
                                    user=user
                                    parent_of=parent_of()
                                    admins=admins.get()
                                    on_promote=on_promote
                                    on_demote=on_demote
                                    on_move=on_move
                                    on_delete=on_delete
                                    collapsed=collapsed
                                    dragging=dragging
                                    drop_target=drop_target
                                    scrolled=scrolled
                                />
                            </For>

                            <Show when=move || !items.get().is_empty() || showing_ghosts()>
                                <tr class="border-0">
                                    // Sticky under the header, which is exactly
                                    // 2.5rem tall, so scrolling through a long
                                    // tree never loses which group is on screen.
                                    // The label is muted on its own: `opacity`
                                    // on the cell would fade the background too
                                    // and let the rows scroll through it.
                                    <td
                                        colspan="7"
                                        style=under_head
                                        class="sticky py-2 px-3 text-xs font-semibold uppercase z-[9] bg-base-200 text-base-content/60"
                                    >
                                        {move || format!("No admin · {}", total.get())}
                                    </td>
                                </tr>
                            </Show>

                            // Nothing hangs off a non-admin, so never a chevron.
                            <For
                                each=move || items.get()
                                key=|user: &AdminUserInfo| row_key(user, false, false)
                                let:user
                            >
                                <UserRow
                                    matched=false
                                    branching=false
                                    selected=selected
                                    user=user
                                    parent_of=parent_of()
                                    admins=admins.get()
                                    on_promote=on_promote
                                    on_demote=on_demote
                                    on_move=on_move
                                    on_delete=on_delete
                                    collapsed=collapsed
                                    dragging=dragging
                                    drop_target=drop_target
                                    scrolled=scrolled
                                />
                            </For>

                            <Show when=showing_ghosts>
                                {(0..GHOST_ROWS)
                                    .map(|_| view! { <GhostRow columns=7 /> })
                                    .collect_view()}
                            </Show>
                        </tbody>
                    </table>

                    <Show when=move || loading.get() && !items.get().is_empty()>
                        <p class="flex gap-2 justify-center items-center py-4 text-base-content/60">
                            <span class="loading loading-spinner loading-sm"></span>
                            "Loading…"
                        </p>
                    </Show>
                </div>

                <Show when=move || {
                    !loading.get() && items.get().is_empty() && admin_count() == 0
                        && !debounced.get().trim().is_empty()
                }>
                    <p class="py-6 text-center text-base-content/60">
                        "No accounts match that search."
                    </p>
                </Show>
            </div>

            <ConfirmDialog
                open=confirm_open
                title=Signal::derive(dialog_title)
                message=Signal::derive(dialog_message)
                confirm_label="Confirm"
                confirm_class=Signal::derive(move || {
                    match pending.get() {
                        Some(Pending::Demote(_)) => "btn-warning".to_string(),
                        _ => "btn-error".to_string(),
                    }
                })
                extra=move || {
                    view! {
                        <Show when=has_orphans>
                            <div class="flex flex-col gap-2 mb-6">
                                <p class="text-sm text-base-content/70">
                                    {move || orphan_summary()}
                                </p>
                                <label class="flex gap-3 items-start cursor-pointer">
                                    <input
                                        type="radio"
                                        name="orphans"
                                        class="mt-1 radio radio-sm"
                                        prop:checked=move || orphans.get() == AdminOrphans::Demote
                                        on:change=move |_| orphans.set(AdminOrphans::Demote)
                                    />
                                    <span class="text-sm">
                                        "Demote them, and everyone below them"
                                    </span>
                                </label>
                                <label class="flex gap-3 items-start cursor-pointer">
                                    <input
                                        type="radio"
                                        name="orphans"
                                        class="mt-1 radio radio-sm"
                                        prop:checked=move || orphans.get() == AdminOrphans::Reparent
                                        on:change=move |_| orphans.set(AdminOrphans::Reparent)
                                    />
                                    <span class="text-sm">{move || reparent_label()}</span>
                                </label>
                            </div>
                        </Show>
                    }
                }
                on_confirm=confirmed
            />
        </Show>
    }
}
