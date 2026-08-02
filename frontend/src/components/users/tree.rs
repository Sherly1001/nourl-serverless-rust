//! The admin chain as the page holds it: ancestry, ordering, folding, search.
//!
//! Every function here is pure and takes the list the server sent, so the
//! shapes the page draws can be checked without a browser or a database.
//!
//! The permission rules — [`may_manage`] and [`may_move_under`] — deliberately
//! restate what `backend/src/routes/admin.rs` enforces. They are not the
//! authority and cannot be: the server decides by walking `promoted_by` in
//! Mongo, and answers 403 whatever this file thinks. They exist so the page
//! does not offer a button that is going to be refused. If the server's rules
//! change, these change with them.

use std::collections::{HashMap, HashSet};

use shared::{AdminUserInfo, UserInfo};

/// Guards every walk of the chain. The tree is a handful of levels deep in
/// practice; the bound only exists so a cycle — which nothing but a hand-edited
/// database can produce — cannot spin forever.
const MAX_WALK: usize = 64;

/// Maps an account id to whoever promoted them, so ancestry can be walked
/// without another request. Built from `admins`, since nothing else can be an
/// interior node of the tree.
pub fn parents(admins: &[AdminUserInfo]) -> HashMap<String, String> {
    admins
        .iter()
        .filter_map(|a| a.promoted_by.clone().map(|parent| (a.id.clone(), parent)))
        .collect()
}

/// Whether `ancestor` sits somewhere above `start` in the chain.
fn descends_from(start: Option<&str>, ancestor: &str, parent_of: &HashMap<String, String>) -> bool {
    let mut at = start;
    for _ in 0..MAX_WALK {
        match at {
            Some(id) if id == ancestor => return true,
            Some(id) => at = parent_of.get(id).map(String::as_str),
            None => return false,
        }
    }
    false
}

/// Whether the signed-in admin may act on this row. The server decides for
/// real by walking `promoted_by` in the database; this walks the copy the page
/// already holds, only to keep the UI from offering a button that would 403.
pub fn may_manage(
    actor: &UserInfo,
    row: &AdminUserInfo,
    parent_of: &HashMap<String, String>,
) -> bool {
    if !actor.is_admin || actor.id == row.id {
        return false;
    }
    if !row.is_admin {
        // In no subtree, so any admin may act on them — the same reason the
        // server allows it: otherwise a fresh signup could never be promoted.
        return true;
    }
    descends_from(row.promoted_by.as_deref(), &actor.id, parent_of)
}

/// Whether this row may be moved under `parent`, mirroring the server's rules
/// for a re-parent: the destination has to be an admin the caller controls, and
/// must not sit inside the branch being moved.
pub fn may_move_under(
    actor: &UserInfo,
    row: &AdminUserInfo,
    parent: &AdminUserInfo,
    parent_of: &HashMap<String, String>,
) -> bool {
    if !row.is_admin || parent.id == row.id || !parent.is_admin {
        return false;
    }
    // Already there.
    if row.promoted_by.as_deref() == Some(parent.id.as_str()) {
        return false;
    }
    // Only inside your own part of the chain — yourself included, which is the
    // ordinary "take responsibility for them" move.
    if parent.id != actor.id && !descends_from(parent.promoted_by.as_deref(), &actor.id, parent_of)
    {
        return false;
    }
    // And never under one of their own, which would cut the branch loose.
    !descends_from(parent.promoted_by.as_deref(), &row.id, parent_of)
}

/// Everyone below `id`, so a demote confirmation can say how many accounts are
/// about to lose the flag. Counted from the loaded tree rather than asked for,
/// since the page already holds every admin.
pub fn descendants<'a>(id: &str, admins: &'a [AdminUserInfo]) -> Vec<&'a AdminUserInfo> {
    let parent_of = parents(admins);
    admins
        .iter()
        .filter(|row| row.id != id && descends_from(row.promoted_by.as_deref(), id, &parent_of))
        .collect()
}

/// The admins in the order they should be drawn: each root followed by its
/// branch, depth first, so a row always appears under its parent.
///
/// Whatever order the server returned is kept between siblings, which is how
/// the sort control reaches a tree that cannot itself be sorted flat. Anything
/// unreachable from a root — only possible from a hand-edited database — is
/// appended rather than dropped, because a page that silently omits an admin is
/// worse than one that shows an odd-looking row.
pub fn tree_order(admins: &[AdminUserInfo]) -> Vec<AdminUserInfo> {
    let mut children: HashMap<&str, Vec<&AdminUserInfo>> = HashMap::new();
    let mut roots: Vec<&AdminUserInfo> = Vec::new();
    for row in admins {
        match row.promoted_by.as_deref() {
            Some(parent) => children.entry(parent).or_default().push(row),
            None => roots.push(row),
        }
    }

    let mut ordered: Vec<AdminUserInfo> = Vec::with_capacity(admins.len());
    let mut stack: Vec<&AdminUserInfo> = roots.into_iter().rev().collect();
    while let Some(row) = stack.pop() {
        ordered.push(row.clone());
        if let Some(kids) = children.get(row.id.as_str()) {
            stack.extend(kids.iter().rev());
        }
    }

    if ordered.len() < admins.len() {
        let seen: std::collections::HashSet<String> =
            ordered.iter().map(|r| r.id.clone()).collect();
        let stray: Vec<AdminUserInfo> = admins
            .iter()
            .filter(|row| !seen.contains(&row.id))
            .cloned()
            .collect();
        ordered.extend(stray);
    }
    ordered
}

/// Whether a row answers the search. An empty needle matches nothing rather
/// than everything: it is asked "is this one of the hits", and with nothing
/// typed there are no hits. `needle` is already trimmed and lowercased.
pub fn matches(user: &AdminUserInfo, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    [
        Some(&user.username),
        user.display_name.as_ref(),
        user.email.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|field| field.to_lowercase().contains(needle))
}

/// The tree narrowed to the search: every hit, plus the admins above it.
///
/// The server deliberately sends the chain whole — dropping an admin whose name
/// does not match would cut every admin below them loose — so narrowing it is
/// this side's job. Ancestors are kept even when they do not match, because a
/// hit shown without its chain would sit at an indent that points at nothing.
/// An empty needle is not a search and leaves the tree alone.
pub fn search_tree(ordered: &[AdminUserInfo], needle: &str) -> Vec<AdminUserInfo> {
    if needle.is_empty() {
        return ordered.to_vec();
    }
    let parent_of = parents(ordered);
    let mut keep: HashSet<&str> = HashSet::new();
    for hit in ordered.iter().filter(|row| matches(row, needle)) {
        keep.insert(hit.id.as_str());
        let mut at = hit.promoted_by.as_deref();
        for _ in 0..MAX_WALK {
            let Some(id) = at else { break };
            // Already kept means its own ancestors were kept with it.
            if !keep.insert(id) {
                break;
            }
            at = parent_of.get(id).map(String::as_str);
        }
    }
    ordered
        .iter()
        .filter(|row| keep.contains(row.id.as_str()))
        .cloned()
        .collect()
}

/// Whether anything hangs off this row, which is the only thing that earns it a
/// collapse control.
pub fn has_children(id: &str, admins: &[AdminUserInfo]) -> bool {
    admins
        .iter()
        .any(|row| row.promoted_by.as_deref() == Some(id))
}

/// Hides the branch under every collapsed node, keeping the node itself.
///
/// Works on the already-ordered list rather than the tree, because depth-first
/// order means a branch is exactly the run of deeper rows that follows its
/// root — so one pass with a depth watermark is enough, and nesting collapses
/// inside collapses costs nothing extra.
pub fn visible_rows(ordered: &[AdminUserInfo], collapsed: &HashSet<String>) -> Vec<AdminUserInfo> {
    let mut rows = Vec::with_capacity(ordered.len());
    // Set while skipping: everything deeper than this is inside a folded branch.
    let mut hide_below: Option<i32> = None;
    for row in ordered {
        let level = row.admin_level.unwrap_or(0);
        if hide_below.is_some_and(|depth| level > depth) {
            continue;
        }
        hide_below = None;
        rows.push(row.clone());
        if collapsed.contains(&row.id) {
            hide_below = Some(level);
        }
    }
    rows
}

/// The admins that hang directly off the accounts about to be deleted, and are
/// not being deleted themselves. Exactly the accounts the choice is about: the
/// ones deeper down follow whatever happens to these.
pub fn orphaned_admins(doomed: &[AdminUserInfo], admins: &[AdminUserInfo]) -> Vec<AdminUserInfo> {
    let going: HashSet<&str> = doomed.iter().map(|row| row.id.as_str()).collect();
    admins
        .iter()
        .filter(|row| !going.contains(row.id.as_str()))
        .filter(|row| {
            row.promoted_by
                .as_deref()
                .is_some_and(|parent| going.contains(parent))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
pub mod fixtures {
    use super::*;

    pub fn plain(id: &str) -> AdminUserInfo {
        AdminUserInfo {
            id: id.into(),
            username: id.into(),
            display_name: None,
            email: None,
            avatar_url: None,
            is_admin: false,
            admin_level: None,
            promoted_by: None,
            providers: Vec::new(),
            has_password: true,
            created_at: None,
            url_count: 0,
        }
    }

    pub fn admin_row(id: &str, level: i32, parent: Option<&str>) -> AdminUserInfo {
        AdminUserInfo {
            is_admin: true,
            admin_level: Some(level),
            promoted_by: parent.map(String::from),
            ..plain(id)
        }
    }

    pub fn actor(id: &str, is_admin: bool) -> UserInfo {
        UserInfo {
            id: id.into(),
            username: id.into(),
            display_name: None,
            email: None,
            avatar_url: None,
            is_admin,
            is_root: false,
            has_password: true,
        }
    }

    /// root ─┬─ left ── left_child
    ///       └─ right
    pub fn tree() -> Vec<AdminUserInfo> {
        vec![
            admin_row("root", 0, None),
            admin_row("left", 1, Some("root")),
            admin_row("right", 1, Some("root")),
            admin_row("left_child", 2, Some("left")),
        ]
    }

    pub fn row(admins: &[AdminUserInfo], id: &str) -> AdminUserInfo {
        admins.iter().find(|a| a.id == id).unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn an_admin_manages_their_own_subtree_and_nothing_else() {
        let admins = tree();
        let parent_of = parents(&admins);

        // Never your own account.
        assert!(!may_manage(
            &actor("left", true),
            &row(&admins, "left"),
            &parent_of
        ));
        // Down your own branch, at any distance.
        assert!(may_manage(
            &actor("root", true),
            &row(&admins, "left_child"),
            &parent_of
        ));
        assert!(may_manage(
            &actor("left", true),
            &row(&admins, "left_child"),
            &parent_of
        ));
        // Not a sibling branch, even at the same depth.
        assert!(!may_manage(
            &actor("left", true),
            &row(&admins, "right"),
            &parent_of
        ));
        assert!(!may_manage(
            &actor("right", true),
            &row(&admins, "left_child"),
            &parent_of
        ));
        // Not upward.
        assert!(!may_manage(
            &actor("left_child", true),
            &row(&admins, "root"),
            &parent_of
        ));
        // An ordinary account belongs to no subtree, so anyone may act on it.
        assert!(may_manage(
            &actor("left_child", true),
            &plain("nobody"),
            &parent_of
        ));
        // And someone with no flag at all may act on nobody.
        assert!(!may_manage(
            &actor("nobody", false),
            &plain("someone"),
            &parent_of
        ));
    }

    #[test]
    fn a_move_is_offered_only_where_the_server_would_allow_it() {
        let admins = tree();
        let parent_of = parents(&admins);
        let can = |actor_id: &str, row_id: &str, parent_id: &str| {
            may_move_under(
                &actor(actor_id, true),
                &row(&admins, row_id),
                &row(&admins, parent_id),
                &parent_of,
            )
        };

        // The root may hand its left branch to the right one.
        assert!(can("root", "left", "right"));
        // Nobody may put a branch under itself or its own child.
        assert!(!can("root", "left", "left"));
        assert!(!can("root", "left", "left_child"));
        // Already there, so there is nothing to offer.
        assert!(!can("root", "left_child", "left"));
        // `left` controls its own child but not where else it could go.
        assert!(!can("left", "left_child", "right"));
        // Already under `left`, so there is nothing to offer.
        assert!(!can("left", "left_child", "left"));
        // An ordinary account is not a place to hang an admin.
        assert!(!may_move_under(
            &actor("root", true),
            &row(&admins, "left"),
            &plain("nobody"),
            &parent_of
        ));
    }

    #[test]
    fn a_demote_confirmation_counts_the_whole_branch() {
        let admins = tree();
        let ids = |id: &str| {
            let mut names: Vec<&str> = descendants(id, &admins)
                .iter()
                .map(|a| a.id.as_str())
                .collect();
            names.sort_unstable();
            names
        };
        assert_eq!(ids("root"), ["left", "left_child", "right"]);
        assert_eq!(ids("left"), ["left_child"]);
        // A leaf takes nobody with it.
        assert!(ids("left_child").is_empty());
    }

    #[test]
    fn the_tree_draws_each_branch_under_its_parent() {
        // Deliberately not in tree order: the server sorts by whatever the
        // column headers asked for, and the shape is imposed here.
        let admins = vec![
            admin_row("left_child", 2, Some("left")),
            admin_row("right", 1, Some("root")),
            admin_row("root", 0, None),
            admin_row("left", 1, Some("root")),
        ];
        let ordered: Vec<String> = tree_order(&admins).iter().map(|r| r.id.clone()).collect();
        assert_eq!(ordered, ["root", "right", "left", "left_child"]);
    }

    /// Two seeded admins are legal, and a forest must render as a forest.
    #[test]
    fn a_second_root_is_drawn_rather_than_dropped() {
        let admins = vec![
            admin_row("a", 0, None),
            admin_row("a_child", 1, Some("a")),
            admin_row("b", 0, None),
        ];
        let ordered = tree_order(&admins);
        assert_eq!(ordered.len(), 3);
        let ids: Vec<&str> = ordered.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["a", "a_child", "b"]);
    }

    /// An admin whose parent is missing is unreachable from any root. Only a
    /// hand-edited database can do it, and dropping them would hide an account
    /// that still holds the flag.
    #[test]
    fn an_orphan_is_still_shown() {
        let admins = vec![
            admin_row("root", 0, None),
            admin_row("stray", 1, Some("gone")),
        ];
        let ids: Vec<String> = tree_order(&admins).iter().map(|r| r.id.clone()).collect();
        assert_eq!(ids, ["root", "stray"]);
    }

    #[test]
    fn only_a_node_with_a_branch_can_be_collapsed() {
        let admins = tree();
        assert!(has_children("root", &admins));
        assert!(has_children("left", &admins));
        assert!(!has_children("right", &admins));
        assert!(!has_children("left_child", &admins));
    }

    #[test]
    fn collapsing_a_node_hides_its_branch_but_not_itself() {
        let ordered = tree_order(&tree());
        let ids = |collapsed: &[&str]| {
            let folded: HashSet<String> = collapsed.iter().map(|s| s.to_string()).collect();
            visible_rows(&ordered, &folded)
                .iter()
                .map(|r| r.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&[]), ["root", "left", "left_child", "right"]);
        // The collapsed node stays; only what hangs off it goes.
        assert_eq!(ids(&["left"]), ["root", "left", "right"]);
        // A sibling branch is untouched by the depth watermark.
        assert_eq!(ids(&["root"]), ["root"]);
        // Folding inside a fold is not a special case.
        assert_eq!(ids(&["left", "root"]), ["root"]);
    }

    #[test]
    fn searching_the_tree_keeps_the_hits_and_the_chain_above_them() {
        let admins = tree();
        let found = search_tree(&admins, "left_child");
        let ids: Vec<&str> = found.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, ["root", "left", "left_child"]);

        // A hit brings its chain, not its branch: nothing under `right`, and
        // the other root's branch is gone entirely.
        let ids: Vec<String> = search_tree(&admins, "right")
            .iter()
            .map(|row| row.id.clone())
            .collect();
        assert_eq!(ids, ["root", "right"]);
        assert_eq!(search_tree(&admins, "nobody").len(), 0);
        // Not a search: the tree is left whole.
        assert_eq!(search_tree(&admins, "").len(), admins.len());
    }

    #[test]
    fn a_row_matches_on_any_of_its_names() {
        let mut user = plain("someone");
        user.display_name = Some("Big Name".into());
        user.email = Some("me@example.com".into());
        assert!(matches(&user, "some"));
        assert!(matches(&user, "big"));
        assert!(matches(&user, "example.com"));
        assert!(!matches(&user, "elsewhere"));
        // Nothing typed is not a match-all.
        assert!(!matches(&user, ""));
    }

    /// Only the rows directly below, and only the ones staying: a branch deeper
    /// down follows its own parent, and a row that is itself being deleted is
    /// not something to re-parent.
    #[test]
    fn the_orphans_are_the_admins_directly_below_who_are_staying() {
        let admins = tree();
        let doomed = vec![row(&admins, "left")];
        let ids: Vec<String> = orphaned_admins(&doomed, &admins)
            .iter()
            .map(|row| row.id.clone())
            .collect();
        assert_eq!(ids, ["left_child"]);

        let both = vec![row(&admins, "left"), row(&admins, "left_child")];
        assert!(orphaned_admins(&both, &admins).is_empty());
        assert!(orphaned_admins(&[row(&admins, "right")], &admins).is_empty());
    }
}
