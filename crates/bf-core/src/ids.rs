//! Typed row ids. Each is a transparent wrapper over the `bigint` key, so JSON, query strings
//! and SQL see a bare number, while the compiler refuses a topic id where a user id is expected.
//!
//! Convert at the edge with `From<i64>` / `.get()`; the wrappers are `Copy` and `Ord`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema, sqlx::Type,
        )]
        #[serde(transparent)]
        #[sqlx(transparent)]
        pub struct $name(pub i64);

        impl $name {
            /// The raw key, for the places that still speak `i64`.
            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = std::num::ParseIntError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse().map(Self)
            }
        }

        impl From<i64> for $name {
            fn from(v: i64) -> Self {
                Self(v)
            }
        }

        impl From<$name> for i64 {
            fn from(v: $name) -> i64 {
                v.0
            }
        }

        impl PartialEq<i64> for $name {
            fn eq(&self, o: &i64) -> bool {
                self.0 == *o
            }
        }

        impl PartialEq<$name> for i64 {
            fn eq(&self, o: &$name) -> bool {
                *self == o.0
            }
        }
    };
}

typed_id!(
    /// `users.id`.
    UserId
);
typed_id!(
    /// `groups.id`.
    GroupId
);
typed_id!(
    /// `nodes.id`: a forum, category or any other tree node.
    ForumId
);
typed_id!(
    /// `topics.id`.
    TopicId
);
typed_id!(
    /// `posts.id`.
    PostId
);

impl GroupId {
    /// The built-in groups every install starts with.
    pub const GUESTS: GroupId = GroupId(1);
    pub const MEMBERS: GroupId = GroupId(2);
    pub const MODERATORS: GroupId = GroupId(3);
    pub const ADMINISTRATORS: GroupId = GroupId(4);
}

/// The raw keys of a slice of typed ids, for the APIs that still take `&[i64]`.
pub fn raw<T: Copy + Into<i64>>(ids: &[T]) -> Vec<i64> {
    ids.iter().map(|id| (*id).into()).collect()
}

/// Typed ids from raw keys, the other way round.
pub fn typed<T: From<i64>>(raw: impl IntoIterator<Item = i64>) -> Vec<T> {
    raw.into_iter().map(T::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Body {
        topic: Option<TopicId>,
        post: PostId,
    }

    #[test]
    fn json_is_a_bare_number() {
        assert_eq!(serde_json::to_string(&UserId(42)).unwrap(), "42");
        assert_eq!(serde_json::from_str::<GroupId>("7").unwrap(), GroupId(7));
        assert_eq!(serde_json::to_string(&vec![ForumId(1), ForumId(2)]).unwrap(), "[1,2]");
        let b: Body = serde_json::from_str(r#"{"topic":null,"post":5}"#).unwrap();
        assert_eq!(b, Body { topic: None, post: PostId(5) });
        assert_eq!(serde_json::to_string(&b).unwrap(), r#"{"topic":null,"post":5}"#);
    }

    #[test]
    fn sqlx_type_is_the_integer() {
        use sqlx::{Type, postgres::Postgres};
        assert_eq!(<UserId as Type<Postgres>>::type_info(), <i64 as Type<Postgres>>::type_info());
        assert!(<TopicId as Type<Postgres>>::compatible(&<i64 as Type<Postgres>>::type_info()));
        assert_eq!(<Vec<GroupId> as Type<Postgres>>::type_info(), <Vec<i64> as Type<Postgres>>::type_info());
    }

    #[test]
    fn conversions_and_display() {
        let u: UserId = 9.into();
        assert_eq!(i64::from(u), 9);
        assert_eq!(u.to_string(), "9");
        assert_eq!(raw(&[UserId(1), UserId(2)]), vec![1, 2]);
        assert_eq!(typed::<GroupId>([3, 4]), vec![GroupId(3), GroupId(4)]);
        assert_eq!("12".parse::<PostId>().unwrap().get(), 12);
    }
}
