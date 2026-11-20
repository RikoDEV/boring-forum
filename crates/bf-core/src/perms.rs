//! Permission resolution: effective = union over the user's groups of each
//! node's grants, then per-user restrictions and moderator scopes.

use std::collections::HashMap;

use crate::ids::{ForumId, GroupId, UserId};

/// Node permission bits stored in `node_permissions.perms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Perms(pub u32);

impl Perms {
    pub const VIEW: Perms = Perms(1);
    pub const READ: Perms = Perms(1 << 1);
    pub const CREATE: Perms = Perms(1 << 2);
    pub const REPLY: Perms = Perms(1 << 3);
    pub const ATTACH: Perms = Perms(1 << 4);
    pub const MODERATE: Perms = Perms(1 << 5);
    /// Open the files attached to posts here (pictures shown in a post only need READ).
    pub const DOWNLOAD: Perms = Perms(1 << 6);
    pub const NONE: Perms = Perms(0);
    pub const ALL: Perms = Perms((1 << 7) - 1);
    /// What reading a forum normally includes: seeing it, reading it and opening its files.
    pub const READER: Perms = Perms(Self::VIEW.0 | Self::READ.0 | Self::DOWNLOAD.0);
    /// Bits a silenced user keeps: they can look but not write.
    const READ_ONLY: Perms = Self::READER;

    pub const fn union(self, o: Perms) -> Perms {
        Perms(self.0 | o.0)
    }
    pub const fn intersect(self, o: Perms) -> Perms {
        Perms(self.0 & o.0)
    }
    pub const fn has(self, o: Perms) -> bool {
        self.0 & o.0 == o.0
    }
}

/// What plugins' `user.can` filter has taken away from one member: whole actions (`reply`, `chat`,
/// `flag`, ...) and, for the forum permissions, per forum. Deny-only: it never adds a right.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Denials {
    /// Actions denied everywhere: the permission names (`view`, `read`, `create`, `reply`,
    /// `attach`, `download`, `moderate`) and the action names the server maps requests to.
    pub actions: Vec<String>,
    /// Actions denied in one forum.
    pub nodes: std::collections::HashMap<i64, Vec<String>>,
}

impl Denials {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty() && self.nodes.is_empty()
    }

    pub fn denies(&self, action: &str) -> bool {
        self.actions.iter().any(|a| a == action)
    }

    fn bits(&self, node: ForumId) -> u32 {
        let names = |list: &[String]| {
            list.iter().fold(0, |acc, n| {
                acc | match n.as_str() {
                    "view" => Perms::VIEW.0,
                    "read" => Perms::READ.0,
                    "create" => Perms::CREATE.0,
                    "reply" => Perms::REPLY.0,
                    "attach" => Perms::ATTACH.0,
                    "download" => Perms::DOWNLOAD.0,
                    "moderate" => Perms::MODERATE.0,
                    _ => 0,
                }
            })
        };
        names(&self.actions) | self.nodes.get(&node.0).map_or(0, |l| names(l))
    }
}

/// Who is asking. Guests are the built-in guest group with no user id.
#[derive(Debug, Clone, Default)]
pub struct Actor {
    /// What plugins took away from this member (see [`Denials`]), applied by [`effective`].
    pub denied: std::sync::Arc<Denials>,
    pub user_id: Option<UserId>,
    pub groups: Vec<GroupId>,
    pub is_admin: bool,
    pub silenced: bool,
    /// Nodes this user moderates through a scoped moderator assignment.
    pub moderated_nodes: Vec<ForumId>,
}

/// Grants for one node: group id → bits. Loaded once and cached per node.
pub type NodeGrants = HashMap<GroupId, Perms>;

pub fn effective(actor: &Actor, node_id: ForumId, grants: &NodeGrants) -> Perms {
    if actor.is_admin {
        return Perms::ALL;
    }
    let mut p = actor.groups.iter().filter_map(|g| grants.get(g)).fold(Perms::NONE, |a, b| a.union(*b));
    if !actor.denied.is_empty() {
        p = Perms(p.0 & !actor.denied.bits(node_id));
    }
    if actor.moderated_nodes.contains(&node_id) && p.has(Perms::VIEW) {
        p = Perms::ALL;
    }
    // Writing always needs an account, whatever the guest group was granted.
    if actor.user_id.is_none() {
        p = p.intersect(Perms::READ_ONLY);
    }
    if actor.silenced && !p.has(Perms::MODERATE) {
        p = p.intersect(Perms::READ_ONLY);
    }
    // Every other right needs VIEW.
    if !p.has(Perms::VIEW) {
        return Perms::NONE;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUESTS: GroupId = GroupId(1);
    const MEMBERS: GroupId = GroupId(2);
    const VIP: GroupId = GroupId(3);
    const NODE: ForumId = ForumId(9);

    fn grants() -> NodeGrants {
        HashMap::from([
            (GUESTS, Perms::VIEW.union(Perms::READ)),
            (MEMBERS, Perms::VIEW.union(Perms::READ).union(Perms::REPLY)),
            (VIP, Perms::CREATE),
        ])
    }

    #[test]
    fn plugin_denials_only_take_rights_away() {
        let mut actor = member(&[MEMBERS]);
        let n = grants();
        assert!(effective(&actor, NODE, &n).has(Perms::REPLY));
        actor.denied = std::sync::Arc::new(Denials { actions: vec!["reply".into()], ..Default::default() });
        assert!(!effective(&actor, NODE, &n).has(Perms::REPLY));
        assert!(effective(&actor, NODE, &n).has(Perms::READ), "the rest stays");
        // Per forum, and only for the named forum.
        actor.denied = std::sync::Arc::new(Denials {
            nodes: std::collections::HashMap::from([(NODE.0, vec!["read".to_owned(), "view".to_owned()])]),
            ..Default::default()
        });
        assert_eq!(effective(&actor, NODE, &n), Perms::NONE);
        // Administrators are exempt, and a denial never adds a right.
        actor.is_admin = true;
        assert_eq!(effective(&actor, NODE, &n), Perms::ALL);
        let mut guest = member(&[GUESTS]);
        guest.denied = std::sync::Arc::new(Denials { actions: vec!["moderate".into()], ..Default::default() });
        assert!(!effective(&guest, NODE, &n).has(Perms::MODERATE));
    }

    fn member(groups: &[GroupId]) -> Actor {
        Actor { user_id: Some(UserId(1)), groups: groups.to_vec(), ..Default::default() }
    }

    #[test]
    fn union_of_groups() {
        let p = effective(&member(&[MEMBERS, VIP]), NODE, &grants());
        assert!(p.has(Perms::REPLY) && p.has(Perms::CREATE));
        assert!(!p.has(Perms::MODERATE));
    }

    #[test]
    fn rights_without_view_are_void() {
        assert_eq!(effective(&member(&[VIP]), NODE, &grants()), Perms::NONE);
    }

    #[test]
    fn guests_never_write() {
        let mut g = grants();
        g.insert(GUESTS, Perms::ALL);
        let guest = Actor { groups: vec![GUESTS], ..Default::default() };
        let p = effective(&guest, NODE, &g);
        assert!(p.has(Perms::READ) && !p.has(Perms::REPLY) && !p.has(Perms::MODERATE));
    }

    #[test]
    fn silenced_users_read_only() {
        let a = Actor { silenced: true, ..member(&[MEMBERS]) };
        let p = effective(&a, NODE, &grants());
        assert!(p.has(Perms::READ) && !p.has(Perms::REPLY));
    }

    #[test]
    fn scoped_moderator_and_admin() {
        let m = Actor { moderated_nodes: vec![NODE], ..member(&[MEMBERS]) };
        assert!(effective(&m, NODE, &grants()).has(Perms::MODERATE));
        assert!(!effective(&m, ForumId(10), &grants()).has(Perms::MODERATE));
        let a = Actor { is_admin: true, ..member(&[]) };
        assert_eq!(effective(&a, NODE, &NodeGrants::new()), Perms::ALL);
    }
}
