//! The admin chain as a value. Who is above, who is below and who may act are
//! all walks over `promoted_by`, so an action takes one snapshot and answers
//! them in memory rather than a `$graphLookup` per account.

use std::collections::{HashMap, HashSet};

use shared::{AdminOrphans, BulkAction};

/// Deep enough for any real hierarchy, shallow enough that a hand-edited cycle
/// cannot turn a lookup into a long walk.
pub const MAX_CHAIN_DEPTH: usize = 32;

/// Every admin and whoever vouches for them; ordinary accounts are absent,
/// which is what makes them nobody's to reach.
pub struct Forest {
    parents: HashMap<String, Option<String>>,
    children: HashMap<String, Vec<String>>,
}

impl Forest {
    pub fn new(rows: impl IntoIterator<Item = (String, Option<String>)>) -> Self {
        let parents: HashMap<String, Option<String>> = rows.into_iter().collect();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        for (id, parent) in &parents {
            if let Some(parent) = parent {
                children.entry(parent.clone()).or_default().push(id.clone());
            }
        }
        // Document order is not an order; the writes are grouped and the
        // counts are sets, but tests read better against a stable one.
        for ids in children.values_mut() {
            ids.sort();
        }
        Self { parents, children }
    }

    pub fn is_admin(&self, id: &str) -> bool {
        self.parents.contains_key(id)
    }

    /// Whoever vouches for `id`, or `None` for a root and for anyone unflagged.
    pub fn parent(&self, id: &str) -> Option<&str> {
        self.parents.get(id)?.as_deref()
    }

    /// Nearest first, stopping at [`MAX_CHAIN_DEPTH`] so a cycle terminates.
    pub fn ancestors(&self, id: &str) -> Vec<String> {
        let mut up = Vec::new();
        let mut at = self.parent(id);
        while let Some(parent) = at {
            if up.len() == MAX_CHAIN_DEPTH {
                break;
            }
            up.push(parent.to_string());
            at = self.parent(parent);
        }
        up
    }

    /// Everyone below `id`, excluding `id` itself.
    pub fn subtree(&self, id: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        let mut queue: Vec<(&str, usize)> = vec![(id, 0)];
        while let Some((at, depth)) = queue.pop() {
            if depth == MAX_CHAIN_DEPTH {
                continue;
            }
            for child in self.children.get(at).into_iter().flatten() {
                if seen.insert(child.as_str()) {
                    found.push(child.clone());
                    queue.push((child, depth + 1));
                }
            }
        }
        found
    }

    /// `id`'s children that are not themselves going, which are the only ones
    /// a re-parenting moves.
    fn children_outside(&self, id: &str, going: &HashSet<&str>) -> Vec<String> {
        self.children
            .get(id)
            .into_iter()
            .flatten()
            .filter(|child| !going.contains(child.as_str()))
            .cloned()
            .collect()
    }

    /// Whether `actor` may act on `target`: the chain above `target` must pass
    /// through `actor`, so nobody reaches sideways into a peer's branch. An
    /// unflagged account is in no subtree, which is how the tree grows at all.
    pub fn may_manage(&self, actor: &str, target: &str) -> bool {
        !self.is_admin(target) || self.ancestors(target).iter().any(|up| up == actor)
    }
}

/// Every write one action implies, grouped so the count does not follow from
/// how many accounts were named.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Admins losing the flag, the named ones included.
    pub demoted: Vec<String>,
    /// Admins below the named ones, which the caller reports separately.
    pub demoted_below: u64,
    /// Where each moved admin lands, grouped by their new parent; `None` is a
    /// root.
    pub reparented: Vec<(Option<String>, Vec<String>)>,
    /// Promotions, grouped by the admin vouching for them.
    pub promoted: Vec<(String, Vec<String>)>,
    /// Accounts to remove, links and all.
    pub deleted: Vec<String>,
}

impl Plan {
    /// Accounts that moved, however far each of them climbed.
    pub fn moved(&self) -> u64 {
        self.reparented
            .iter()
            .map(|(_, ids)| ids.len() as u64)
            .sum()
    }
}

/// What `action` does to `targets`, decided whole. Order is what the sequential
/// version used depth for: a demoted branch is the union of the subtrees, and a
/// re-parented admin lands on its nearest ancestor that is not itself going.
pub fn plan(
    forest: &Forest,
    actor: &str,
    targets: &[String],
    action: BulkAction,
    orphans: AdminOrphans,
) -> Plan {
    if action == BulkAction::Promote {
        return Plan {
            promoted: grouped(targets.iter().map(|id| {
                let parent = forest.parent(id).unwrap_or(actor).to_string();
                (parent, id.clone())
            })),
            ..Default::default()
        };
    }

    let named: Vec<&String> = targets.iter().filter(|id| forest.is_admin(id)).collect();
    let going: HashSet<&str> = named.iter().map(|id| id.as_str()).collect();

    let (demoted, reparented) = match orphans {
        AdminOrphans::Demote => {
            let mut falling: Vec<String> = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            for id in &named {
                for below in std::iter::once((*id).clone()).chain(forest.subtree(id)) {
                    if seen.insert(below.clone()) {
                        falling.push(below);
                    }
                }
            }
            (falling, Vec::new())
        }
        AdminOrphans::Reparent => {
            let moved = going.iter().flat_map(|id| {
                forest
                    .children_outside(id, &going)
                    .into_iter()
                    .map(|child| (survivor(forest, &child, &going), child))
            });
            // A deletion takes the row with the flag on it, so only a demotion
            // writes one.
            let demoted = match action {
                BulkAction::Delete => Vec::new(),
                _ => named.iter().map(|id| (*id).clone()).collect(),
            };
            (demoted, grouped(moved))
        }
    };

    Plan {
        demoted_below: demoted.len().saturating_sub(named.len()) as u64,
        demoted,
        reparented,
        promoted: Vec::new(),
        deleted: match action {
            BulkAction::Delete => targets.to_vec(),
            _ => Vec::new(),
        },
    }
}

/// A promotion onto a chosen parent, which is the one thing the bulk route
/// never asks for: it moves an admin as well as flagging an account.
pub fn promote_under(parent: &str, ids: &[String]) -> Plan {
    Plan {
        promoted: vec![(parent.to_string(), ids.to_vec())],
        ..Default::default()
    }
}

/// The nearest ancestor of `id` that survives the action, or a root when none
/// does — the same place a deepest-first walk would have left them.
fn survivor(forest: &Forest, id: &str, going: &HashSet<&str>) -> Option<String> {
    forest
        .ancestors(id)
        .into_iter()
        .find(|up| !going.contains(up.as_str()))
}

/// Pairs into one entry per key, each carrying its ids in a stable order.
fn grouped<K: Ord + std::hash::Hash + Clone>(
    pairs: impl IntoIterator<Item = (K, String)>,
) -> Vec<(K, Vec<String>)> {
    let mut groups: HashMap<K, Vec<String>> = HashMap::new();
    for (key, id) in pairs {
        groups.entry(key).or_default().push(id);
    }
    let mut out: Vec<(K, Vec<String>)> = groups.into_iter().collect();
    for (_, ids) in &mut out {
        ids.sort();
        ids.dedup();
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forest(rows: &[(&str, Option<&str>)]) -> Forest {
        Forest::new(
            rows.iter()
                .map(|(id, parent)| (id.to_string(), parent.map(String::from))),
        )
    }

    /// The tree used throughout: `root` vouches for `a` and `peer`, `a` for
    /// `b`, `b` for `c`.
    fn tree() -> Forest {
        forest(&[
            ("root", None),
            ("a", Some("root")),
            ("b", Some("a")),
            ("c", Some("b")),
            ("peer", Some("root")),
        ])
    }

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn sorted(mut ids: Vec<String>) -> Vec<String> {
        ids.sort();
        ids
    }

    #[test]
    fn the_chain_reads_both_ways() {
        let tree = tree();
        assert_eq!(tree.ancestors("c"), ids(&["b", "a", "root"]));
        assert_eq!(sorted(tree.subtree("a")), ids(&["b", "c"]));
        assert!(tree.subtree("c").is_empty());
        assert!(!tree.is_admin("nobody"));
    }

    /// The rule that keeps an admin out of a peer's branch, and lets any of
    /// them promote an account that is in no branch at all.
    #[test]
    fn reach_runs_down_your_own_branch_only() {
        let tree = tree();
        assert!(tree.may_manage("root", "c"));
        assert!(tree.may_manage("a", "c"));
        assert!(!tree.may_manage("peer", "c"));
        assert!(!tree.may_manage("b", "a"));
        assert!(tree.may_manage("peer", "unflagged"));
    }

    /// A cycle can only come from a hand-edited document, and it must not hang
    /// the walk that finds it.
    #[test]
    fn a_cycle_stops_at_the_depth_cap() {
        let looped = forest(&[("x", Some("y")), ("y", Some("x"))]);
        assert_eq!(looped.ancestors("x").len(), MAX_CHAIN_DEPTH);
        assert_eq!(sorted(looped.subtree("x")), ids(&["x", "y"]));
    }

    #[test]
    fn promoting_puts_an_unflagged_account_under_the_actor() {
        let plan = plan(
            &tree(),
            "root",
            &ids(&["new"]),
            BulkAction::Promote,
            AdminOrphans::Demote,
        );
        assert_eq!(plan.promoted, vec![("root".to_string(), ids(&["new"]))]);
    }

    /// Promoting an admin again is how they are moved, so their vouching stays
    /// where it is rather than snapping to whoever asked.
    #[test]
    fn promoting_an_admin_leaves_them_where_they_hang() {
        let plan = plan(
            &tree(),
            "root",
            &ids(&["c"]),
            BulkAction::Promote,
            AdminOrphans::Demote,
        );
        assert_eq!(plan.promoted, vec![("b".to_string(), ids(&["c"]))]);
    }

    #[test]
    fn demoting_takes_the_whole_branch_with_it() {
        let plan = plan(
            &tree(),
            "root",
            &ids(&["a"]),
            BulkAction::Demote,
            AdminOrphans::Demote,
        );
        assert_eq!(sorted(plan.demoted.clone()), ids(&["a", "b", "c"]));
        assert_eq!(plan.demoted_below, 2);
        assert!(plan.reparented.is_empty());
    }

    /// Two named accounts on one branch are one branch: the overlap is counted
    /// once, as a cascade that ran twice would have modified it once.
    #[test]
    fn a_nested_selection_counts_the_overlap_once() {
        let plan = plan(
            &tree(),
            "root",
            &ids(&["a", "b"]),
            BulkAction::Demote,
            AdminOrphans::Demote,
        );
        assert_eq!(sorted(plan.demoted.clone()), ids(&["a", "b", "c"]));
        assert_eq!(plan.demoted_below, 1);
    }

    /// What a deepest-first walk did one level at a time: `c` climbs past `b`
    /// and `a`, both going, and lands on `root`, counted once for the climb.
    #[test]
    fn a_nested_selection_moves_each_admin_to_its_nearest_survivor() {
        let plan = plan(
            &tree(),
            "root",
            &ids(&["a", "b"]),
            BulkAction::Demote,
            AdminOrphans::Reparent,
        );
        assert_eq!(
            plan.reparented,
            vec![(Some("root".to_string()), ids(&["c"]))]
        );
        assert_eq!(plan.moved(), 1);
        assert_eq!(sorted(plan.demoted.clone()), ids(&["a", "b"]));
        assert_eq!(plan.demoted_below, 0);
    }

    /// Nothing above survives, so the branch keeps its standing as roots.
    #[test]
    fn a_branch_with_no_survivor_above_it_becomes_roots() {
        let plan = plan(
            &tree(),
            "root",
            &ids(&["root", "a"]),
            BulkAction::Demote,
            AdminOrphans::Reparent,
        );
        assert_eq!(plan.reparented, vec![(None, ids(&["b", "peer"]))]);
    }

    /// The row carrying the flag is being deleted, so there is no flag to
    /// write — only the branch that climbs out first.
    #[test]
    fn deleting_with_reparenting_writes_no_demotion() {
        let plan = plan(
            &tree(),
            "root",
            &ids(&["a"]),
            BulkAction::Delete,
            AdminOrphans::Reparent,
        );
        assert!(plan.demoted.is_empty());
        assert_eq!(
            plan.reparented,
            vec![(Some("root".to_string()), ids(&["b"]))]
        );
        assert_eq!(plan.deleted, ids(&["a"]));
    }

    /// Deleting counts the branch below, never the named account: it is gone,
    /// not demoted.
    #[test]
    fn deleting_with_demotion_counts_only_the_branch() {
        let plan = plan(
            &tree(),
            "root",
            &ids(&["a"]),
            BulkAction::Delete,
            AdminOrphans::Demote,
        );
        assert_eq!(plan.demoted_below, 2);
        assert_eq!(plan.deleted, ids(&["a"]));
    }

    /// An ordinary account is in no branch, so deleting it cascades nothing.
    #[test]
    fn deleting_an_unflagged_account_touches_the_chain_not_at_all() {
        let plan = plan(
            &tree(),
            "root",
            &ids(&["nobody"]),
            BulkAction::Delete,
            AdminOrphans::Demote,
        );
        assert!(plan.demoted.is_empty());
        assert!(plan.reparented.is_empty());
        assert_eq!(plan.deleted, ids(&["nobody"]));
    }
}
