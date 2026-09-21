//! One account as a table row: the indent and its guides, the chevron, the
//! admin switch, the move menu and the row's own actions.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use shared::AdminUserInfo;
use wasm_bindgen::JsCast;

use crate::auth::use_auth;
use crate::components::avatar::{Avatar, usable_url};
use crate::components::tooltip::Tooltip;
use crate::dropdown::dismiss_on_outside_click;
use crate::list::short_datetime;

use super::tree::{may_manage, may_move_under, may_resign};

/// What `<For>` keys a row by. The id alone would let Leptos reuse a node
/// across a move and keep drawing the old shape; `branching` and `matched` are
/// read once when the row is built, so they belong in the key too.
pub fn row_key(user: &AdminUserInfo, branching: bool, matched: bool) -> String {
    format!(
        "{}@{}@{}@{branching}@{matched}",
        user.id,
        user.admin_level.unwrap_or(-1),
        user.promoted_by.as_deref().unwrap_or(""),
    )
}

/// Whether a chevron-less row still reserves the width. Every admin does, or a
/// childless root starts at a different depth from a branching one — and the
/// tree is read by where a row starts.
pub fn needs_gutter(is_admin: bool) -> bool {
    is_admin
}

/// Space kept between a menu and the button it hangs off, and the most menu
/// that will ever be drawn before it starts scrolling inside itself.
const MENU_GAP: f64 = 4.0;
const MENU_MAX_HEIGHT: f64 = 256.0;

/// Where a row's menu sits, in viewport coordinates. `fixed`, or the scrolling
/// table body clips it; dropped on whichever side has room and capped to it.
/// Plain numbers, so the placement can be checked without a browser.
pub fn menu_placement(trigger: (f64, f64, f64), viewport: (f64, f64)) -> String {
    let (top, bottom, edge_right) = trigger;
    let (width, height) = viewport;
    let right = (width - edge_right).max(0.0);
    let below = height - bottom - MENU_GAP;
    let above = top - MENU_GAP;
    let (edge, offset, room) = if below >= above {
        ("top", bottom + MENU_GAP, below)
    } else {
        ("bottom", height - top + MENU_GAP, above)
    };
    let cap = room.clamp(0.0, MENU_MAX_HEIGHT);
    format!("position:fixed;right:{right}px;{edge}:{offset}px;max-height:{cap}px")
}

/// How an account signs in, for the "Signs in with" column.
pub fn sign_in_methods(user: &AdminUserInfo) -> Vec<String> {
    let mut methods: Vec<String> = user.providers.clone();
    if user.has_password {
        methods.insert(0, "password".into());
    }
    methods
}

/// One account, in whichever group. `admin_level` drives the indent, so an
/// account with no rank sits flush and the same markup serves both.
#[component]
#[allow(clippy::too_many_arguments)]
pub fn UserRow(
    user: AdminUserInfo,
    parent_of: HashMap<String, String>,
    /// Every admin, for the move menu and the cascade count.
    admins: Vec<AdminUserInfo>,
    on_promote: Callback<AdminUserInfo>,
    on_demote: Callback<AdminUserInfo>,
    on_move: Callback<(AdminUserInfo, String)>,
    on_delete: Callback<AdminUserInfo>,
    /// Which branches are folded, shared with the page so the chevron and the
    /// row list agree.
    collapsed: RwSignal<HashSet<String>>,
    /// The row currently under the pointer's grip, and the row it is hovering
    /// over. Both live on the page because a drag spans two rows.
    dragging: RwSignal<Option<AdminUserInfo>>,
    drop_target: RwSignal<Option<String>>,
    /// Bumped by the page every time the list scrolls, which is what closes an
    /// open menu — it is pinned to the viewport and would otherwise stay put
    /// while its row moves away underneath it.
    scrolled: RwSignal<u32>,
    /// Highlights a row the search actually matched. The rows above it are on
    /// screen to hold the chain together, not because they answered anything.
    matched: bool,
    /// Whether anything is still hanging off this row, which is what earns it a
    /// chevron. Decided by the page, against the tree the search left behind —
    /// a row whose whole branch was filtered away has nothing to unfold.
    branching: bool,
    /// Ids ticked for a bulk action, shared with the page so the row and the
    /// "select all" box agree.
    selected: RwSignal<HashSet<String>>,
) -> impl IntoView {
    let auth = use_auth();
    let row = StoredValue::new(user.clone());
    let id = StoredValue::new(user.id.clone());
    let name = user
        .display_name
        .clone()
        .unwrap_or_else(|| user.username.clone());
    let methods = sign_in_methods(&user);
    // Untracked throughout: a row is a snapshot, and the page already refetches
    // and redraws the whole list when the signed-in user changes.
    let mine = auth.user.get_untracked().is_some_and(|me| me.id == user.id);
    let manageable = auth
        .user
        .get_untracked()
        .is_some_and(|me| may_manage(&me, &user, &parent_of));
    // Your own row is not manageable, but you may still stand down from it.
    let resignable = auth
        .user
        .get_untracked()
        .is_some_and(|me| may_resign(&me, &user));
    let is_admin = user.is_admin;
    let level = user.admin_level.unwrap_or(0);
    // Only a row the server would let this admin re-parent is worth picking up.
    let movable = is_admin && manageable;

    // Whether this row would accept the drag in flight. Reactive because it is
    // re-answered on every `dragover`, against whichever row is being carried.
    // `StoredValue` so the closure stays `Copy` and every handler can take one.
    let chain = StoredValue::new(parent_of.clone());
    let accepts = move || {
        let (Some(me), Some(carried)) = (auth.user.get_untracked(), dragging.get()) else {
            return false;
        };
        chain.with_value(|parent_of| may_move_under(&me, &carried, &row.get_value(), parent_of))
    };
    let hovered = move || drop_target.get().as_deref() == Some(id.get_value().as_str());
    let lifted = move || {
        dragging
            .get()
            .is_some_and(|carried| carried.id == id.get_value())
    };
    let row_class = move || {
        let base = if matched { "bg-warning/10" } else { "" };
        let drag = if lifted() {
            " opacity-40"
        } else if hovered() && accepts() {
            " bg-primary/10 outline outline-2 -outline-offset-2 outline-primary/50"
        } else {
            ""
        };
        format!("{base}{drag}")
    };

    let on_dragstart = move |ev: leptos::ev::DragEvent| {
        if let Some(data) = ev.data_transfer() {
            data.set_effect_allowed("move");
            // Firefox refuses to start a drag without payload, even unused.
            let _ = data.set_data("text/plain", &id.get_value());
        }
        dragging.set(Some(row.get_value()));
    };
    let on_dragend = move |_| {
        dragging.set(None);
        drop_target.set(None);
    };
    let on_dragover = move |ev: leptos::ev::DragEvent| {
        if !accepts() {
            return;
        }
        // Only a cancelled `dragover` marks a valid drop zone, so refusing to
        // cancel is how an invalid target says no.
        ev.prevent_default();
        // `dragover` repeats for as long as the pointer is held here, so the
        // signal is only written when the answer actually changes.
        if !hovered() {
            drop_target.set(Some(id.get_value()));
        }
    };
    let on_dragleave = move |ev: leptos::ev::DragEvent| {
        // Bubbles from the cells too, so a move onto a child of this row reads
        // as leaving it. Only a departure to something outside counts.
        let into_child = ev
            .current_target()
            .and_then(|here| here.dyn_into::<web_sys::Node>().ok())
            .zip(ev.related_target())
            .is_some_and(|(here, gone_to)| here.contains(gone_to.dyn_ref::<web_sys::Node>()));
        if hovered() && !into_child {
            drop_target.set(None);
        }
    };
    let on_drop = move |ev: leptos::ev::DragEvent| {
        ev.prevent_default();
        // Asked before the signal is cleared, since `accepts` reads it.
        let carried = accepts().then(|| dragging.get_untracked()).flatten();
        dragging.set(None);
        drop_target.set(None);
        if let Some(carried) = carried {
            on_move.run((carried, id.get_value()));
        }
    };
    // `StoredValue` rather than plain `String`s: these labels sit inside
    // `<Show>` bodies, which are `Fn` and so cannot consume what they capture.
    let username = user.username.clone();
    let select_label = StoredValue::new(format!("Select {}", user.username));
    let move_label = StoredValue::new(format!("Move {}", user.username));
    let delete_label = StoredValue::new(format!("Delete {}", user.username));
    let admin_label = StoredValue::new(format!("Admin: {}", user.username));
    // What each control does, for the hover hint. An icon on its own says
    // nothing, and the action differs by row: a switch that grants for one
    // account withdraws for the next.
    let admin_hint = match (manageable, resignable, is_admin) {
        (true, _, true) => "Remove admin",
        (true, _, false) => "Make admin",
        // Standing down reads differently from taking someone else's flag: it
        // needs nobody's permission, and it is not something to do by accident.
        (false, true, _) => "Give up admin",
        (false, false, _) if mine && is_admin => {
            "The top admin's flag can only be removed in the database"
        }
        (false, false, _) => "Not yours to change",
    };
    let delete_hint = if manageable {
        "Delete account"
    } else {
        "Not yours to delete"
    };
    let fold_hint = "Collapse or expand";
    let fold_label = StoredValue::new(format!("Collapse {}", user.username));

    // Where this row could be moved to, worked out from the tree already
    // loaded rather than by asking the server what it would accept.
    let destinations: Vec<AdminUserInfo> = auth
        .user
        .get_untracked()
        .map(|me| {
            admins
                .iter()
                .filter(|parent| may_move_under(&me, &user, parent, &parent_of))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let (menu_open, set_menu_open) = signal(false);
    // Worked out when the menu opens, since it is pinned to the viewport rather
    // than to the row.
    let menu_style = RwSignal::new(String::new());
    let menu_root: NodeRef<leptos::html::Div> = NodeRef::new();
    dismiss_on_outside_click(menu_root, set_menu_open);
    // A fixed menu does not travel with its row, so scrolling the list closes
    // it rather than leaving it stranded next to somebody else's row.
    Effect::new(move |seen: Option<u32>| {
        let tick = scrolled.get();
        if seen.is_some_and(|before| before != tick) {
            set_menu_open.set(false);
        }
        tick
    });
    let on_menu_click = move |ev: leptos::ev::MouseEvent| {
        if menu_open.get_untracked() {
            set_menu_open.set(false);
            return;
        }
        let placed = ev
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .zip(web_sys::window())
            .map(|(button, win)| {
                let size = |v: Result<wasm_bindgen::JsValue, _>| {
                    v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0)
                };
                let rect = button.get_bounding_client_rect();
                menu_placement(
                    (rect.top(), rect.bottom(), rect.right()),
                    (size(win.inner_width()), size(win.inner_height())),
                )
            })
            .unwrap_or_default();
        menu_style.set(placed);
        set_menu_open.set(true);
    };

    view! {
        <tr
            class=row_class
            draggable=if movable { "true" } else { "false" }
            on:dragstart=on_dragstart
            on:dragend=on_dragend
            on:dragover=on_dragover
            on:dragleave=on_dragleave
            on:drop=on_drop
        >
            <td>
                <Show when=move || manageable>
                    <input
                        type="checkbox"
                        class="checkbox checkbox-sm"
                        aria-label=move || select_label.get_value()
                        prop:checked=move || selected.get().contains(&id.get_value())
                        on:change=move |_| {
                            let this = id.get_value();
                            selected
                                .update(|set| {
                                    if !set.remove(&this) {
                                        set.insert(this);
                                    }
                                });
                        }
                    />
                </Show>
            </td>

            <td class=if movable { "cursor-grab active:cursor-grabbing" } else { "" }>
                <span class="flex items-stretch -my-2">
                    {(0..level)
                        .map(|_| {
                            view! {
                                <span class="w-5 border-r shrink-0 border-base-content/15"></span>
                            }
                        })
                        .collect_view()}
                    <span class="flex overflow-hidden gap-2 items-center py-2 pl-1 min-w-0">
                        <Show
                            when=move || branching
                            fallback=move || {
                                needs_gutter(is_admin)
                                    .then(|| {
                                        view! { <span class="shrink-0 size-6"></span> }
                                    })
                            }
                        >
                            <Tooltip text=fold_hint class="inline-flex" only_when_clipped=false>
                                <button
                                    class="rounded shrink-0 btn btn-text btn-xs btn-square"
                                    aria-label=move || fold_label.get_value()
                                    aria-expanded=move || {
                                        (!collapsed.get().contains(&id.get_value())).to_string()
                                    }
                                    on:click=move |_| {
                                        collapsed
                                            .update(|folded| {
                                                if !folded.remove(&id.get_value()) {
                                                    folded.insert(id.get_value());
                                                }
                                            })
                                    }
                                >
                                    <span class=move || {
                                        let folded = collapsed.get().contains(&id.get_value());
                                        let icon = if folded {
                                            "icon-[tabler--chevron-right]"
                                        } else {
                                            "icon-[tabler--chevron-down]"
                                        };
                                        format!("{icon} size-4")
                                    }></span>
                                </button>
                            </Tooltip>
                        </Show>
                        <Avatar
                            url=usable_url(user.avatar_url.as_deref())
                            name=name.clone()
                            size="size-6"
                        />
                        <span class="flex flex-col min-w-0">
                            <span class="flex gap-2 items-center min-w-0">
                                <Tooltip text=username.clone() class="block truncate">
                                    {username.clone()}
                                </Tooltip>
                                <Show when=move || mine>
                                    <span class="badge badge-soft badge-sm">"you"</span>
                                </Show>
                            </span>
                            {user
                                .email
                                .clone()
                                .map(|email| {
                                    view! {
                                        <Tooltip
                                            text=email.clone()
                                            class="block text-xs truncate text-base-content/50"
                                        >
                                            {email.clone()}
                                        </Tooltip>
                                    }
                                })}
                        </span>
                    </span>
                </span>
            </td>
            <td>
                <span class="flex flex-wrap gap-1">
                    {methods
                        .into_iter()
                        .map(|m| view! { <span class="badge badge-soft badge-sm">{m}</span> })
                        .collect_view()}
                </span>
            </td>
            <td>{user.url_count}</td>
            <td class="whitespace-nowrap opacity-70">{short_datetime(user.created_at.as_ref())}</td>
            <td>
                <span class="flex gap-2 items-center">
                    <Tooltip text=admin_hint class="inline-flex" only_when_clipped=false>
                        <input
                            type="checkbox"
                            class="switch switch-sm"
                            aria-label=move || admin_label.get_value()
                            prop:checked=is_admin
                            disabled=!(manageable || resignable)
                            on:change=move |ev: leptos::ev::Event| {
                                if let Some(box_) = ev
                                    .target()
                                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                {
                                    box_.set_checked(is_admin);
                                }
                                let row = row.get_value();
                                if row.is_admin {
                                    on_demote.run(row);
                                } else {
                                    on_promote.run(row);
                                }
                            }
                        />
                    </Tooltip>
                    <Show when=move || is_admin>
                        <span class="text-xs whitespace-nowrap opacity-70">
                            {format!("L{level}")}
                        </span>
                    </Show>
                </span>
            </td>
            <td class="text-right">
                <div class="inline-flex relative gap-1 items-center" node_ref=menu_root>
                    <Show when={
                        let empty = destinations.is_empty();
                        move || is_admin && manageable && !empty
                    }>
                        <Tooltip
                            text="Move under another admin"
                            class="inline-flex"
                            only_when_clipped=false
                        >
                            <button
                                class="btn btn-text btn-sm btn-square"
                                aria-label=move || move_label.get_value()
                                aria-haspopup="menu"
                                on:click=on_menu_click
                            >
                                <span class="icon-[tabler--git-branch] size-4"></span>
                            </button>
                        </Tooltip>
                    </Show>
                    <Show when=move || menu_open.get()>
                        <ul
                            class="overflow-y-auto z-30 w-56 rounded-lg border shadow-lg bg-base-100 border-base-content/10 motion-preset-slide-down motion-duration-200"
                            style=move || menu_style.get()
                            role="menu"
                        >
                            <li class="sticky top-0 py-2 px-3 text-xs uppercase border-b bg-base-100 text-base-content/50 border-base-content/10">
                                "Promoted by"
                            </li>
                            {destinations
                                .clone()
                                .into_iter()
                                .map(|parent| {
                                    let label = parent.username.clone();
                                    let parent_id = parent.id.clone();
                                    view! {
                                        <li>
                                            <button
                                                class="py-2 px-3 w-full text-left truncate hover:bg-base-200"
                                                role="menuitem"
                                                on:click=move |_| {
                                                    set_menu_open.set(false);
                                                    on_move.run((row.get_value(), parent_id.clone()));
                                                }
                                            >
                                                {label}
                                            </button>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    </Show>
                    <Tooltip text=delete_hint class="inline-flex" only_when_clipped=false>
                        <button
                            class="btn btn-text btn-sm btn-square text-error"
                            aria-label=move || delete_label.get_value()
                            disabled=!manageable
                            on:click=move |_| on_delete.run(row.get_value())
                        >
                            <span class="icon-[tabler--trash] size-4"></span>
                        </button>
                    </Tooltip>
                </div>
            </td>
        </tr>
    }
}

#[cfg(test)]
mod tests {
    use super::super::tree::fixtures::*;
    use super::*;

    /// Two roots, one with a branch and one without, have to start at the same
    /// depth — otherwise the childless one reads as sitting above the other.
    #[test]
    fn every_admin_reserves_the_chevrons_width_even_at_the_top_level() {
        assert!(needs_gutter(true), "a root with no children still lines up");
        assert!(!needs_gutter(false), "accounts with no rank keep the edge");
    }

    #[test]
    fn the_row_key_changes_when_a_move_changes_the_row() {
        let before = admin_row("mover", 1, Some("root"));
        let moved = admin_row("mover", 2, Some("left"));
        assert_ne!(
            row_key(&before, false, false),
            row_key(&moved, false, false)
        );
        // Same row, same key — a reload must not redraw the whole tree.
        assert_eq!(
            row_key(&before, false, false),
            row_key(&admin_row("mover", 1, Some("root")), false, false)
        );
        // And a row with no rank at all still gets a stable key.
        assert_eq!(
            row_key(&plain("nobody"), false, false),
            row_key(&plain("nobody"), false, false)
        );
    }

    #[test]
    fn the_menu_drops_on_whichever_side_has_room() {
        // Near the top: below, and free to use its full height.
        assert_eq!(
            menu_placement((100.0, 132.0, 900.0), (1000.0, 800.0)),
            "position:fixed;right:100px;top:136px;max-height:256px"
        );
        // Near the bottom: flipped above, measured from the same edge.
        assert_eq!(
            menu_placement((700.0, 732.0, 900.0), (1000.0, 800.0)),
            "position:fixed;right:100px;bottom:104px;max-height:256px"
        );
        // And in a short window neither side has 16rem: the roomier one still
        // wins, capped to what is there so the menu scrolls inside itself
        // rather than off-screen.
        assert_eq!(
            menu_placement((100.0, 132.0, 900.0), (1000.0, 200.0)),
            "position:fixed;right:100px;bottom:104px;max-height:96px"
        );
    }

    /// A hit deep in the chain has to bring its ancestors with it, or it is
    /// drawn indented under a parent that is not on the page.
    #[test]
    fn the_row_key_changes_when_a_row_gains_its_first_child() {
        // Otherwise the node is reused and the new parent never grows a chevron.
        let parent = admin_row("parent", 1, Some("root"));
        assert_ne!(
            row_key(&parent, false, false),
            row_key(&parent, true, false)
        );
        // And a row that starts or stops answering the search.
        assert_ne!(row_key(&parent, true, false), row_key(&parent, true, true));
    }

    #[test]
    fn sign_in_methods_lead_with_password() {
        let with = |has_password, providers: &[&str]| AdminUserInfo {
            has_password,
            providers: providers.iter().map(|p| p.to_string()).collect(),
            ..plain("a")
        };
        assert_eq!(
            sign_in_methods(&with(true, &["github"])),
            ["password", "github"]
        );
        assert_eq!(sign_in_methods(&with(false, &["google"])), ["google"]);
        // An account with neither can only exist through direct database
        // editing, and the column should say so by being empty.
        assert!(sign_in_methods(&with(false, &[])).is_empty());
    }
}
