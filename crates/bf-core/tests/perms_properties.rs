//! Permission property tests.
//!
//! `effective` is the one place permissions are resolved, and every list, search, feed, sitemap,
//! notification and API response is supposed to agree with it. Example-based tests check the
//! cases someone thought of; these check invariants over randomly generated groups, grants and
//! actors, which is where the combinations nobody thought of live.
//!
//! Each property is written as a rule that must hold for *every* input. When one fails,
//! proptest shrinks the counterexample to something small enough to paste into a bug report.

use bf_core::ids::{ForumId, GroupId, UserId};
use bf_core::perms::{Actor, NodeGrants, Perms, effective};
use proptest::prelude::*;

/// The six real bits, for generating grants and for iterating.
const BITS: [Perms; 7] =
    [Perms::VIEW, Perms::READ, Perms::CREATE, Perms::REPLY, Perms::ATTACH, Perms::DOWNLOAD, Perms::MODERATE];

/// Group ids from the same small space the grants use.
fn any_group() -> impl Strategy<Value = GroupId> {
    (1i64..6).prop_map(GroupId)
}

/// Node ids small enough that moderator scopes and queried nodes collide often.
fn any_node() -> impl Strategy<Value = ForumId> {
    (1i64..4).prop_map(ForumId)
}

prop_compose! {
    /// A permission mask over the defined bits only; the resolver must never invent a bit
    /// outside this range.
    fn any_perms()(bits in 0u32..(1 << 7)) -> Perms {
        Perms(bits)
    }
}

prop_compose! {
    /// Grants for a node: a small set of group ids, each with its own mask.
    fn any_grants()(entries in prop::collection::vec((any_group(), any_perms()), 0..6)) -> NodeGrants {
        entries.into_iter().collect()
    }
}

prop_compose! {
    /// An actor: possibly a guest, possibly an admin, possibly silenced, with group memberships
    /// and moderator scopes drawn from the same small id space as the grants.
    fn any_actor()(
        user_id in prop::option::of((1i64..100).prop_map(UserId)),
        groups in prop::collection::vec(any_group(), 0..4),
        is_admin in any::<bool>(),
        silenced in any::<bool>(),
        moderated_nodes in prop::collection::vec(any_node(), 0..3),
    ) -> Actor {
        Actor { user_id, groups, is_admin, silenced, moderated_nodes, ..Default::default() }
    }
}

/// The specification, written independently of the implementation: what the rules say the answer
/// should be. A property test that re-implements the code under test proves nothing, so this is
/// deliberately phrased as the rules of the permission model rather than as a copy of `effective`.
fn specified(actor: &Actor, node: ForumId, grants: &NodeGrants) -> Perms {
    if actor.is_admin {
        return Perms::ALL;
    }
    // Union over the groups the actor is in.
    let mut bits = Perms::NONE;
    for g in &actor.groups {
        if let Some(p) = grants.get(g) {
            bits = bits.union(*p);
        }
    }
    // A scoped moderator gets everything in the nodes they moderate, but only where they can
    // see the node at all.
    if actor.moderated_nodes.contains(&node) && bits.has(Perms::VIEW) {
        bits = Perms::ALL;
    }
    // Looking includes opening a forum's files, which needs no account either.
    let read_only = Perms::VIEW.union(Perms::READ).union(Perms::DOWNLOAD);
    // Writing needs an account, whatever the guest group was granted.
    if actor.user_id.is_none() {
        bits = bits.intersect(read_only);
    }
    // A silenced user may look but not write, unless they are a moderator here.
    if actor.silenced && !bits.has(Perms::MODERATE) {
        bits = bits.intersect(read_only);
    }
    // Everything else depends on being able to view the node.
    if !bits.has(Perms::VIEW) {
        return Perms::NONE;
    }
    bits
}

proptest! {
    /// The resolver agrees with the specification for every input.
    #[test]
    fn matches_the_specification(actor in any_actor(), node in any_node(), grants in any_grants()) {
        prop_assert_eq!(effective(&actor, node, &grants), specified(&actor, node, &grants));
    }

    /// No bit outside the defined set is ever returned, so a stored mask with stray high bits
    /// cannot grant something that does not exist.
    #[test]
    fn never_invents_a_bit(actor in any_actor(), node in any_node(), grants in any_grants()) {
        let p = effective(&actor, node, &grants);
        prop_assert_eq!(p.intersect(Perms::ALL), p, "resolved {:?} has bits outside Perms::ALL", p);
    }

    /// Every right implies VIEW. This is what lets a query filter on one bit and be sure the
    /// row is visible at all.
    #[test]
    fn any_right_implies_view(actor in any_actor(), node in any_node(), grants in any_grants()) {
        let p = effective(&actor, node, &grants);
        prop_assert!(p == Perms::NONE || p.has(Perms::VIEW), "resolved {:?} without VIEW", p);
    }

    /// A guest never gets a write right, however generous the guest group's grant is. The
    /// property that stops a misconfigured permission matrix turning into anonymous posting.
    #[test]
    fn guests_never_write(mut actor in any_actor(), node in any_node(), grants in any_grants()) {
        actor.user_id = None;
        actor.is_admin = false;
        let p = effective(&actor, node, &grants);
        for bit in [Perms::CREATE, Perms::REPLY, Perms::ATTACH, Perms::MODERATE] {
            prop_assert!(!p.has(bit), "a guest resolved to {:?}", p);
        }
    }

    /// A silenced non-moderator never gets a write right, which is the whole point of silencing.
    ///
    /// Moderators are exempt by design: `effective` keeps the full mask when MODERATE is set, so
    /// silencing a moderator does not quietly disarm them. MODERATE can arrive either from a
    /// scope or from a group grant, so both are excluded here rather than just the scope.
    #[test]
    fn silenced_users_cannot_write(mut actor in any_actor(), node in any_node(), grants in any_grants()) {
        actor.silenced = true;
        actor.is_admin = false;
        actor.moderated_nodes.clear();
        let p = effective(&actor, node, &grants);
        prop_assume!(!p.has(Perms::MODERATE));
        for bit in [Perms::CREATE, Perms::REPLY, Perms::ATTACH] {
            prop_assert!(!p.has(bit), "a silenced user resolved to {:?}", p);
        }
    }

    /// Adding a group never takes a right away. Permissions are a union, so a user who joins a
    /// group must not lose access -- a non-monotonic resolver would make group changes unsafe.
    #[test]
    fn adding_a_group_never_removes_a_right(
        actor in any_actor(),
        extra in any_group(),
        node in any_node(),
        grants in any_grants(),
    ) {
        let before = effective(&actor, node, &grants);
        let mut more = actor.clone();
        more.groups.push(extra);
        let after = effective(&more, node, &grants);
        // Only meaningful where the extra group did not newly grant VIEW and flip the node from
        // invisible to visible, which legitimately changes the answer from NONE.
        if before != Perms::NONE {
            prop_assert_eq!(before.intersect(after), before, "{:?} lost rights, becoming {:?}", before, after);
        }
    }

    /// Removing a node's grants entirely leaves nobody but an admin with anything.
    #[test]
    fn an_ungranted_node_is_invisible(mut actor in any_actor(), node in any_node()) {
        actor.is_admin = false;
        prop_assert_eq!(effective(&actor, node, &NodeGrants::new()), Perms::NONE);
    }

    /// An admin always gets everything, in every node, whatever else is true of them.
    #[test]
    fn admins_are_unconditional(mut actor in any_actor(), node in any_node(), grants in any_grants()) {
        actor.is_admin = true;
        prop_assert_eq!(effective(&actor, node, &grants), Perms::ALL);
    }

    /// A moderator scope applies only to the node it names, never to a sibling.
    #[test]
    fn moderator_scope_does_not_leak_to_other_nodes(
        mut actor in any_actor(),
        grants in any_grants(),
    ) {
        actor.is_admin = false;
        actor.moderated_nodes = vec![ForumId(1)];
        let elsewhere = effective(&actor, ForumId(2), &grants);
        // Node 2 is not moderated, so MODERATE can only come from a group grant.
        let from_groups = actor.groups.iter().filter_map(|g| grants.get(g)).fold(Perms::NONE, |a, b| a.union(*b));
        prop_assert!(
            !elsewhere.has(Perms::MODERATE) || from_groups.has(Perms::MODERATE),
            "a scope on node 1 granted MODERATE on node 2: {:?}",
            elsewhere
        );
    }

    /// Resolution is a pure function: the same inputs give the same answer, and the order groups
    /// happen to be listed in does not matter.
    #[test]
    fn resolution_is_order_independent(actor in any_actor(), node in any_node(), grants in any_grants()) {
        let forward = effective(&actor, node, &grants);
        let mut reversed = actor.clone();
        reversed.groups.reverse();
        prop_assert_eq!(effective(&reversed, node, &grants), forward);
    }

    /// The bit helpers behave like set operations, since the resolver is built out of them.
    #[test]
    fn bit_algebra_holds(a in any_perms(), b in any_perms()) {
        prop_assert_eq!(a.union(b), b.union(a));
        prop_assert_eq!(a.intersect(b), b.intersect(a));
        prop_assert!(a.union(b).has(a));
        prop_assert_eq!(a.intersect(a), a);
        prop_assert!(a.has(Perms::NONE));
        prop_assert_eq!(a.union(Perms::NONE), a);
        prop_assert_eq!(a.intersect(Perms::NONE), Perms::NONE);
        for bit in BITS {
            prop_assert_eq!(a.union(bit).has(bit), true);
        }
    }
}
