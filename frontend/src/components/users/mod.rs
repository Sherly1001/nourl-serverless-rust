//! The admin Users page: the chain as a tree, everyone else paged beneath it.

pub mod row;
pub mod text;
pub mod tree;

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::{AdminOrphans, AdminUserInfo, BulkAction};
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
use text::{
    GRACE_DAYS, bulk_delete_warning, bulk_demote_warning, bulk_refusal, bulk_result,
    delete_warning, demote_warning, resign_warning,
};
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

    // Here, not in a row: a drag and a branch both span several rows.
    let collapsed: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let dragging: RwSignal<Option<AdminUserInfo>> = RwSignal::new(None);
    let drop_target: RwSignal<Option<String>> = RwSignal::new(None);
    let scrolled: RwSignal<u32> = RwSignal::new(0);

    let confirm_open = RwSignal::new(false);
    // One dialog, not four: only one can be open, and the rows travel with it.
    let pending: RwSignal<Option<Pending>> = RwSignal::new(None);
    // Ids, not rows: a reload replaces every row but keeps the ids.
    let selected: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    // Measured, not guessed: a fixed offset drifts when the header's height does.
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
        // `try_` throughout: writing a disposed signal is fatal in wasm.
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
    // Waits for the session: `None` means both in flight and signed out.
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

    // Re-reads the lot: a change can move rows between groups and change depth.
    let reload = move || {
        // Leaving ticks behind would arm the next action with finished rows.
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
                    // The held session still claims the flag; refresh before leaving.
                    let stood_down =
                        !is_admin && auth.user.get_untracked().is_some_and(|me| me.id == id);
                    if stood_down {
                        auth.user.try_set(api::me().await.ok());
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().set_hash("/");
                        }
                        return;
                    }
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
    // Reset on every open: the safe answer grants nobody an unearned standing.
    let orphans = RwSignal::new(AdminOrphans::Demote);
    let ask = move |what: Pending| {
        orphans.set(AdminOrphans::Demote);
        pending.set(Some(what));
        confirm_open.set(true);
    };
    let on_demote = Callback::new(move |user: AdminUserInfo| ask(Pending::Demote(vec![user])));
    let on_delete = Callback::new(move |user: AdminUserInfo| ask(Pending::Delete(vec![user])));

    // One request per hundred; all-or-nothing holds within one, not across.
    let run_bulk = move |rows: Vec<AdminUserInfo>, action: BulkAction| {
        let ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
        let choice = orphans.get_untracked();
        spawn_local(async move {
            let word = match action {
                BulkAction::Promote => "promoted",
                BulkAction::Demote => "demoted",
                BulkAction::Delete => "deleted",
            };
            match api::bulk_users(ids, action, choice).await {
                Ok(response) => {
                    toasts.success(bulk_result(
                        word,
                        response.affected,
                        response.demoted,
                        response.reparented,
                    ));
                }
                Err((done, err)) => {
                    // A split selection may have landed in part before the refusal.
                    if done.affected > 0 {
                        toasts.success(bulk_result(
                            word,
                            done.affected,
                            done.demoted,
                            done.reparented,
                        ));
                    }
                    toasts.error(bulk_refusal(
                        &err.message,
                        err.rejected.as_deref().unwrap_or_default(),
                    ));
                }
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
            Pending::Demote(rows) => run_bulk(rows, BulkAction::Demote),
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
            Pending::Delete(rows) => run_bulk(rows, BulkAction::Delete),
        }
    });

    // Whether the branch below leaves a choice to make, and how to word it.
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
    // Named for where they land; a root's children become roots.
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

    // Whether a one-row demote is the signed-in admin resigning.
    let resigning = move || match pending.get() {
        Some(Pending::Demote(rows)) if rows.len() == 1 => {
            auth.user.get().is_some_and(|me| me.id == rows[0].id)
        }
        _ => false,
    };
    let dialog_title = move || match pending.get() {
        Some(Pending::Delete(rows)) if rows.len() == 1 => "Delete account".to_string(),
        Some(Pending::Delete(rows)) => format!("Delete {} accounts", rows.len()),
        _ if resigning() => "Give up admin".to_string(),
        _ => "Remove admin".to_string(),
    };
    let dialog_message = move || match pending.get() {
        None => String::new(),
        Some(Pending::Delete(rows)) if rows.len() == 1 => delete_warning(&rows[0], GRACE_DAYS),
        Some(Pending::Delete(rows)) => bulk_delete_warning(&rows, GRACE_DAYS),
        // Left out while the radios offer the other answer, which it contradicts.
        Some(Pending::Demote(rows)) if rows.len() == 1 => {
            let below = if has_orphans() {
                0
            } else {
                descendants(&rows[0].id, &admins.get()).len()
            };
            if resigning() {
                resign_warning(below)
            } else {
                demote_warning(&rows[0], below)
            }
        }
        Some(Pending::Demote(rows)) if has_orphans() => bulk_demote_warning(&rows, 0),
        Some(Pending::Demote(rows)) => {
            // Only those not already ticked, or the selection is counted twice.
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

    // A hit inside a folded branch would look like the search missed it.
    Effect::new(move |_| {
        let needle = needle();
        if needle.is_empty() {
            return;
        }
        // Untracked, or every promote would re-open the folded branches.
        let shown: HashSet<String> = search_tree(&admins.get_untracked(), &needle)
            .into_iter()
            .map(|row| row.id)
            .collect();
        collapsed.update(|folded| folded.retain(|id| !shown.contains(id)));
    });

    // Two steps: the chevron reads the searched tree, not the folded one.
    let found_rows = Memo::new(move |_| search_tree(&admins.get(), &needle()));
    // The searched tree, or using the chevron would remove it.
    let branching = move |user: &AdminUserInfo| {
        user.is_admin && found_rows.with_untracked(|rows| has_children(&user.id, rows))
    };
    let admin_rows =
        Memo::new(move |_| found_rows.with(|rows| visible_rows(rows, &collapsed.get())));
    // Hits alone while searching: the ancestors are there to hold the shape.
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

    // A checkbox that leads to a 403 is a trap.
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
    // From both groups; a row that has left the list drops out.
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
    // Out of the view: leptosfmt reads a `>` in an attribute as a closing bracket.
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
                                    on:click=move |_| run_bulk(chosen_others(), BulkAction::Promote)
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

                <div
                    class="overflow-auto rounded-lg border max-h-[70vh] border-base-content/10"
                    on:scroll=on_scroll
                >
                    <table class="table table-fixed min-w-[60rem] [&_thead_tr]:border-b-0 [&_td]:px-3">
                        <thead node_ref=head class="sticky top-0 z-10 bg-base-200">
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
                                    <td
                                        colspan="7"
                                        style=under_head
                                        class="sticky py-2 px-3 text-xs font-semibold uppercase z-[9] bg-base-200 text-base-content/60"
                                    >
                                        {move || format!("No admin · {}", total.get())}
                                    </td>
                                </tr>
                            </Show>

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
                                        class="radio radio-sm"
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
                                        class="radio radio-sm"
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
