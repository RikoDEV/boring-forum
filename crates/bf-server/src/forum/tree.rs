//! The forum tree (nodes and permissions), cached in memory and invalidated on change.

use bf_core::AppError;
use bf_core::perms::{Actor, NodeGrants, Perms, effective};
use bf_db::forum::nodes;
use bf_db::forum::nodes::Node;
use bf_db::moderation::watched_words;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub(crate) const GUESTS: i64 = 1;

/// Node tree and permission grants, loaded once and dropped when nodes change.
#[derive(Clone, Default)]
pub struct ForumCache(Arc<RwLock<Option<Arc<Tree>>>>);

pub struct Tree {
    pub nodes: Vec<Node>,
    pub grants: HashMap<bf_core::ids::ForumId, NodeGrants>,
    pub words: Vec<bf_core::watched::Rule>,
}

impl ForumCache {
    pub fn invalidate(&self) {
        *self.0.write().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    pub(crate) async fn get(&self, pool: &PgPool) -> Result<Arc<Tree>, AppError> {
        if let Some(t) = self.0.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone() {
            return Ok(t);
        }
        let tree = Arc::new(Tree {
            nodes: nodes::nodes(pool).await.map_err(AppError::internal)?,
            grants: nodes::node_grants(pool).await.map_err(AppError::internal)?,
            words: watched_words::watched_words(pool)
                .await
                .map_err(AppError::internal)?
                .into_iter()
                .filter_map(|w| {
                    use bf_core::watched::Action;
                    let action = match w.action.as_str() {
                        "block" => Action::Block,
                        "censor" => Action::Censor,
                        "require_approval" => Action::RequireApproval,
                        "replace" => Action::Replace,
                        "link" => Action::Link,
                        "tag" => Action::Tag,
                        "flag" => Action::Flag,
                        "silence" => Action::Silence,
                        _ => return None,
                    };
                    Some(bf_core::watched::Rule { action, pattern: w.pattern, replacement: w.replacement })
                })
                .collect(),
        });
        *self.0.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tree.clone());
        Ok(tree)
    }
}

impl Tree {
    /// The grants that apply to a node: its own, or its parent's when it inherits (see
    /// `nodes::grants_node`).
    pub(crate) fn grants_of(&self, node_id: i64) -> &NodeGrants {
        static EMPTY: std::sync::LazyLock<NodeGrants> = std::sync::LazyLock::new(NodeGrants::new);
        let source = nodes::grants_node(&self.nodes, node_id);
        self.grants.get(&source.into()).unwrap_or(&EMPTY)
    }

    pub(crate) fn perms(&self, actor: &Actor, node_id: i64) -> Perms {
        let Some(node) = self.node(node_id) else { return Perms::NONE };
        let own = effective(actor, node_id.into(), self.grants_of(node_id));
        // Ancestors only gate visibility: a forum is hidden unless every parent can be
        // viewed. Its other rights are its own, so a moderator scoped to a subforum
        // keeps MODERATE there without moderating the parent category.
        let mut id = node.parent_id;
        while let Some(pid) = id {
            let Some(n) = self.node(pid) else { return Perms::NONE }; // dangling parent
            if !effective(actor, n.id.into(), self.grants_of(n.id)).has(Perms::VIEW) {
                return Perms::NONE;
            }
            id = n.parent_id;
        }
        own
    }

    pub(crate) fn node(&self, id: i64) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub(crate) fn readable(&self, actor: &Actor) -> Vec<i64> {
        self.nodes.iter().filter(|n| self.perms(actor, n.id).has(Perms::READ)).map(|n| n.id).collect()
    }
}

pub(crate) fn guest() -> Actor {
    Actor { groups: vec![GUESTS.into()], ..Default::default() }
}

#[cfg(test)]
mod tree_tests {
    use super::*;
    use bf_core::ids::{ForumId, GroupId};

    fn node(id: i64, parent_id: Option<i64>) -> Node {
        Node {
            id,
            parent_id,
            kind: if parent_id.is_some() { "forum" } else { "category" }.into(),
            slug: String::new(),
            title: String::new(),
            description_html: String::new(),
            description_md: String::new(),
            redirect_url: None,
            position: 0,
            topic_count: 0,
            post_count: 0,
            last_activity_at: None,
            default_sort: None,
            seo_title: None,
            seo_description: None,
            list_style: "table".into(),
            rules_html: None,
            prefixes: vec![],
            approve_mode: "none".into(),
            min_trust_level: 0,
            require_tag: false,
            clicks: 0,
            counts_posts: true,
            mode: "discussion".into(),
            auto_close_days: None,
            has_redirects: false,
            icon: None,
            icon_image: None,
            template_md: None,
            theme_id: None,
            inherit_perms: false,
        }
    }

    fn tree(cat_member_bits: Perms) -> Tree {
        let member = Perms::VIEW.union(Perms::READ).union(Perms::REPLY);
        Tree {
            nodes: vec![node(1, None), node(2, Some(1))],
            grants: HashMap::from([
                (ForumId(1), HashMap::from([(GroupId(2), cat_member_bits)])),
                (ForumId(2), HashMap::from([(GroupId(2), member)])),
            ]),
            words: vec![],
        }
    }

    fn member(moderates: &[i64]) -> Actor {
        Actor {
            user_id: Some(bf_core::ids::UserId(9)),
            groups: vec![GroupId(2)],
            moderated_nodes: moderates.iter().copied().map(ForumId).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn scoped_moderator_keeps_moderate_below_a_plain_category() {
        let t = tree(Perms::VIEW);
        assert!(t.perms(&member(&[2]), 2).has(Perms::MODERATE));
        assert!(!t.perms(&member(&[2]), 1).has(Perms::MODERATE));
        assert!(!t.perms(&member(&[]), 2).has(Perms::MODERATE));
    }

    #[test]
    fn hidden_parent_hides_children() {
        let t = tree(Perms::NONE);
        assert_eq!(t.perms(&member(&[2]), 2), Perms::NONE);
    }

    #[test]
    fn child_rights_are_its_own() {
        // The category grants only VIEW, yet members may reply in the forum.
        assert!(tree(Perms::VIEW).perms(&member(&[]), 2).has(Perms::REPLY));
    }

    #[test]
    fn an_inheriting_forum_uses_its_parents_grants() {
        // The forum's own grants (reply) are set aside while it inherits the category's (view only).
        let mut t = tree(Perms::VIEW.union(Perms::READ));
        t.nodes[1].inherit_perms = true;
        let p = t.perms(&member(&[]), 2);
        assert!(p.has(Perms::READ) && !p.has(Perms::REPLY));
        // A change to the category applies at once; a category never inherits from anything.
        let mut t = tree(Perms::VIEW.union(Perms::READ).union(Perms::CREATE));
        t.nodes[1].inherit_perms = true;
        t.nodes[0].inherit_perms = true;
        assert!(t.perms(&member(&[]), 2).has(Perms::CREATE));
        assert_eq!(t.grants_of(1).len(), 1);
    }

    #[test]
    fn inheritance_chains_through_several_levels() {
        let mut t = tree(Perms::VIEW.union(Perms::READ).union(Perms::ATTACH));
        t.nodes.push(node(3, Some(2)));
        t.nodes[1].inherit_perms = true;
        t.nodes[2].inherit_perms = true;
        assert!(t.perms(&member(&[]), 3).has(Perms::ATTACH));
    }
}
