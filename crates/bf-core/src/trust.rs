//! Trust levels: their names, what a member needs for the next one, and how far along they are.
//! The promotion job, the member's progress page and the admin's preview all read this, so what
//! each of them says is what the job decides.
//!
//! The levels are data, edited in the admin panel: level 0 is where everyone starts, each level
//! after it has requirements and is either earned by activity (`auto`) or given by hand. Earned
//! levels come first and in order; a level with a `recent_posts` requirement is kept only while
//! the member stays that active. The evaluator reads a level's numbers through a lookup by key
//! (`trust_level_{n}_{what}`), so the live levels and an admin's proposed ones are judged alike.

use crate::i18n::en;

/// The highest level number there can be.
pub const LIMIT: i16 = 20;
/// A member is "nearly there" when every requirement is at least this far along (percent).
pub const NEARLY_PERCENT: i64 = 80;

/// The requirement columns of a level, by what they measure: `trust_level_{level}_{suffix}`.
pub const SUFFIXES: [&str; 8] =
    ["posts", "days", "topics_read", "reputation", "likes_received", "likes_given", "days_visited", "recent_posts"];

/// The English name of a built-in level; translate it where it is shown (`t.msg(name)`).
pub fn default_name(level: i16) -> Option<&'static str> {
    match level {
        0 => Some(en("Newcomer")),
        1 => Some(en("Member")),
        2 => Some(en("Regular")),
        3 => Some(en("Trusted")),
        4 => Some(en("Leader")),
        _ => None,
    }
}

/// One trust level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Level {
    pub level: i16,
    /// The admin's name for it; empty for the translated default.
    pub name: String,
    /// Earned by activity. Otherwise only a moderator gives it.
    pub auto: bool,
    pub posts: i64,
    pub days: i64,
    pub topics_read: i64,
    pub reputation: i64,
    pub likes_received: i64,
    pub likes_given: i64,
    pub days_visited: i64,
    /// Posts within the recent-activity window needed to reach the level and to keep it; 0 is off.
    pub recent_posts: i64,
}

impl Level {
    /// A requirement by its suffix.
    pub fn get(&self, what: &str) -> i64 {
        match what {
            "posts" => self.posts,
            "days" => self.days,
            "topics_read" => self.topics_read,
            "reputation" => self.reputation,
            "likes_received" => self.likes_received,
            "likes_given" => self.likes_given,
            "days_visited" => self.days_visited,
            "recent_posts" => self.recent_posts,
            _ => 0,
        }
    }

    pub fn set(&mut self, what: &str, value: i64) {
        match what {
            "posts" => self.posts = value,
            "days" => self.days = value,
            "topics_read" => self.topics_read = value,
            "reputation" => self.reputation = value,
            "likes_received" => self.likes_received = value,
            "likes_given" => self.likes_given = value,
            "days_visited" => self.days_visited = value,
            "recent_posts" => self.recent_posts = value,
            _ => {}
        }
    }
}

/// Every level, from 0 up without gaps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Levels(pub Vec<Level>);

impl Default for Levels {
    /// The levels a new site starts with.
    fn default() -> Self {
        let mk = |level, auto, posts, days, topics_read, likes_received, likes_given, days_visited| Level {
            level,
            auto,
            posts,
            days,
            topics_read,
            likes_received,
            likes_given,
            days_visited,
            ..Level::default()
        };
        Levels(vec![
            mk(0, false, 0, 0, 0, 0, 0, 0),
            mk(1, true, 5, 0, 5, 0, 0, 2),
            mk(2, true, 50, 7, 30, 3, 5, 10),
            mk(3, true, 200, 30, 100, 20, 20, 30),
            mk(4, false, 0, 0, 0, 0, 0, 0),
        ])
    }
}

/// The longest name a level can have.
pub const NAME_MAX: usize = 40;

impl Levels {
    /// The levels as stored in the `trust_levels` setting: a JSON array from level 0 up, each
    /// `{"name", "auto", "posts", …}`. Empty text is the default set. Anything that does not
    /// describe a usable set is refused, so a bad value can never reach the promotion job.
    pub fn parse(text: &str) -> Result<Levels, &'static str> {
        if text.trim().is_empty() {
            return Ok(Levels::default());
        }
        let list: Vec<serde_json::Value> = serde_json::from_str(text).map_err(|_| "Trust levels must be a list.")?;
        if list.is_empty() || list.len() > LIMIT as usize + 1 {
            return Err("There must be between 1 and 21 trust levels.");
        }
        let mut out = Vec::with_capacity(list.len());
        for (i, v) in list.iter().enumerate() {
            let mut level = Level { level: i as i16, ..Level::default() };
            v.get("name").and_then(|n| n.as_str()).unwrap_or_default().trim().clone_into(&mut level.name);
            if level.name.chars().count() > NAME_MAX || level.name.contains(['\n', '\r', '<', '>']) {
                return Err("A trust level name is too long or has characters it cannot have.");
            }
            level.auto = v.get("auto").and_then(serde_json::Value::as_bool).unwrap_or(false);
            for what in SUFFIXES {
                let n = v.get(what).and_then(serde_json::Value::as_i64).unwrap_or(0);
                let max = if what == "days" || what == "days_visited" { 3650 } else { 1_000_000 };
                if !(0..=max).contains(&n) {
                    return Err("A trust level requirement is out of range.");
                }
                level.set(what, n);
            }
            if i == 0 && (level.auto || SUFFIXES.iter().any(|w| level.get(w) != 0)) {
                return Err("Level 0 is where everyone starts and has no requirements.");
            }
            if level.auto && !out.last().is_some_and(|p: &Level| p.level == 0 || p.auto) {
                return Err("A level can only be earned when the level below it is earned too.");
            }
            out.push(level);
        }
        Ok(Levels(out))
    }

    /// The text the `trust_levels` setting stores for these levels; empty for the default set.
    pub fn to_setting(&self) -> String {
        if *self == Levels::default() {
            return String::new();
        }
        let list: Vec<serde_json::Value> = self
            .0
            .iter()
            .map(|l| {
                let mut o = serde_json::Map::new();
                o.insert("name".into(), l.name.clone().into());
                o.insert("auto".into(), l.auto.into());
                for what in SUFFIXES {
                    o.insert(what.into(), l.get(what).into());
                }
                serde_json::Value::Object(o)
            })
            .collect();
        serde_json::Value::Array(list).to_string()
    }

    /// The level to use for a stored number: never above the top.
    pub fn clamp(&self, level: i64) -> i16 {
        level.clamp(0, i64::from(self.max_level())) as i16
    }

    /// What a level is called, or `None` for the translated default.
    pub fn custom_name(&self, level: i16) -> Option<&str> {
        self.get(level).map(|l| l.name.as_str()).filter(|n| !n.is_empty())
    }

    /// The requirement of each earned level as a list indexed by the level below it
    /// (`[0]` is what level 1 needs), for the promotion queries.
    pub fn needs(&self, what: &str) -> Vec<i64> {
        (1..=self.max_auto()).map(|n| self.get(n).map_or(0, |l| l.get(what))).collect()
    }

    /// The highest level a member can earn by activity.
    pub fn max_auto(&self) -> i16 {
        let get = |k: &str| self.value(k);
        max_auto(&get)
    }

    pub fn get(&self, level: i16) -> Option<&Level> {
        self.0.iter().find(|l| l.level == level)
    }

    pub fn max_level(&self) -> i16 {
        self.0.last().map_or(0, |l| l.level)
    }

    /// A level's number by key: `trust_level_{n}_{posts|days|…|recent_posts}`, `_auto` (1 or 0)
    /// and `_exists`. Anything else, and a level that is not there, is 0.
    pub fn value(&self, key: &str) -> i64 {
        let Some(rest) = key.strip_prefix("trust_level_") else { return 0 };
        let Some((n, what)) = rest.split_once('_') else { return 0 };
        let Some(level) = n.parse::<i16>().ok().and_then(|n| self.get(n)) else { return 0 };
        match what {
            "exists" => 1,
            "auto" => i64::from(level.auto && level.level > 0),
            other => level.get(other),
        }
    }
}

/// The highest level that exists, read through a lookup.
pub fn max_level(value: &dyn Fn(&str) -> i64) -> i16 {
    (1..=LIMIT).take_while(|n| value(&format!("trust_level_{n}_exists")) == 1).last().unwrap_or(0)
}

/// The highest level a member can earn by activity: the unbroken run of earned levels from 1.
pub fn max_auto(value: &dyn Fn(&str) -> i64) -> i16 {
    (1..=LIMIT).take_while(|n| value(&format!("trust_level_{n}_auto")) == 1).last().unwrap_or(0)
}

/// What a member has done so far, measured the same way for every requirement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Metrics {
    pub posts: i64,
    /// Whole days since joining.
    pub days: i64,
    pub topics_read: i64,
    pub reputation: i64,
    /// Visible posts within the recent-activity window.
    pub recent_posts: i64,
    pub likes_received: i64,
    pub likes_given: i64,
    /// Different days the member has visited.
    pub days_visited: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Posts,
    Days,
    TopicsRead,
    Reputation,
    LikesReceived,
    LikesGiven,
    DaysVisited,
    /// Posts within the last `days` days (to reach a level, and to keep it).
    Recent {
        days: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement {
    pub kind: Kind,
    pub needed: i64,
    pub current: i64,
}

impl Requirement {
    pub fn met(&self) -> bool {
        self.current >= self.needed
    }

    /// Progress from 0 to 100.
    pub fn percent(&self) -> i64 {
        if self.needed <= 0 { 100 } else { (self.current.max(0) * 100 / self.needed).min(100) }
    }
}

/// What a member needs to reach `level` (an earned one), with their progress on each. `value`
/// reads a level's numbers by key (the live levels, or an admin's proposed ones). Requirements
/// left at 0 are not listed. `trust_recent_days` is the window of the recent-activity requirement.
pub fn requirements(value: &dyn Fn(&str) -> i64, level: i16, m: &Metrics) -> Vec<Requirement> {
    if !(1..=max_auto(value)).contains(&level) {
        return Vec::new();
    }
    let need = |suffix: &str| value(&format!("trust_level_{level}_{suffix}"));
    let mut out = Vec::new();
    let mut add = |kind, needed: i64, current: i64| {
        if needed > 0 {
            out.push(Requirement { kind, needed, current });
        }
    };
    add(Kind::Posts, need("posts"), m.posts);
    add(Kind::Days, need("days"), m.days);
    add(Kind::DaysVisited, need("days_visited"), m.days_visited);
    add(Kind::TopicsRead, need("topics_read"), m.topics_read);
    add(Kind::Reputation, need("reputation"), m.reputation);
    add(Kind::LikesReceived, need("likes_received"), m.likes_received);
    add(Kind::LikesGiven, need("likes_given"), m.likes_given);
    add(Kind::Recent { days: value("trust_recent_days") }, need("recent_posts"), m.recent_posts);
    out
}

pub fn all_met(reqs: &[Requirement]) -> bool {
    reqs.iter().all(Requirement::met)
}

/// Not there yet, but every requirement is at least `NEARLY_PERCENT` along.
pub fn nearly_there(reqs: &[Requirement]) -> bool {
    !reqs.is_empty() && !all_met(reqs) && reqs.iter().all(|r| r.percent() >= NEARLY_PERCENT)
}

/// The level a member's activity earns, climbing from `current` while the next level's
/// requirements are all met. Never lowers a level.
pub fn earned_level(value: &dyn Fn(&str) -> i64, current: i16, m: &Metrics) -> i16 {
    let top = max_auto(value);
    let mut level = current.clamp(0, top);
    while level < top && all_met(&requirements(value, level + 1, m)) {
        level += 1;
    }
    level.max(current)
}

/// A level with a recent-activity requirement is kept only while that holds.
pub fn keeps_level(value: &dyn Fn(&str) -> i64, level: i16, m: &Metrics) -> bool {
    if value(&format!("trust_level_{level}_auto")) != 1 {
        return true;
    }
    let needed = value(&format!("trust_level_{level}_recent_posts"));
    needed <= 0 || m.recent_posts >= needed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lookup over a few numbers, with levels 1 to 3 earned and 4 given by hand.
    fn settings(pairs: &'static [(&'static str, i64)]) -> impl Fn(&str) -> i64 {
        move |k| {
            if let Some(found) = pairs.iter().find(|(key, _)| *key == k) {
                return found.1;
            }
            match k {
                "trust_level_1_auto" | "trust_level_2_auto" | "trust_level_3_auto" | "trust_level_1_exists"
                | "trust_level_2_exists" | "trust_level_3_exists" | "trust_level_4_exists" => 1,
                _ => 0,
            }
        }
    }

    #[test]
    fn names() {
        assert_eq!(default_name(0), Some("Newcomer"));
        assert_eq!(default_name(4), Some("Leader"));
        assert_eq!(default_name(5), None);
    }

    #[test]
    fn levels_are_read_by_key() {
        let mut l = Levels::default();
        l.0[2].recent_posts = 7;
        assert_eq!(l.value("trust_level_2_posts"), 50);
        assert_eq!(l.value("trust_level_2_recent_posts"), 7);
        assert_eq!(l.value("trust_level_3_auto"), 1);
        assert_eq!(l.value("trust_level_4_auto"), 0, "a level given by hand is not earned");
        assert_eq!(l.value("trust_level_0_auto"), 0);
        assert_eq!(l.value("trust_level_9_posts"), 0);
        assert_eq!(l.value("trust_level_4_exists"), 1);
        assert_eq!(l.value("something_else"), 0);
        let get = |k: &str| l.value(k);
        assert_eq!((max_level(&get), max_auto(&get)), (4, 3));
    }

    #[test]
    fn levels_round_trip_through_the_setting() {
        assert_eq!(Levels::parse("").unwrap(), Levels::default());
        assert_eq!(Levels::default().to_setting(), "");
        let mut l = Levels::default();
        l.0[1].name = "Regular".into();
        l.0.push(Level { level: 5, name: "Elder".into(), ..Level::default() });
        let text = l.to_setting();
        assert_eq!(Levels::parse(&text).unwrap(), l);
        assert_eq!(l.custom_name(5), Some("Elder"));
        assert_eq!(l.custom_name(2), None);
        assert_eq!(l.clamp(99), 5);
        assert_eq!(l.needs("posts"), vec![5, 50, 200]);
    }

    #[test]
    fn unusable_levels_are_refused() {
        assert!(Levels::parse("not json").is_err());
        assert!(Levels::parse("[]").is_err());
        assert!(Levels::parse(r#"[{"auto":true}]"#).is_err(), "level 0 cannot be earned");
        assert!(Levels::parse(r#"[{},{"posts":-1}]"#).is_err());
        assert!(Levels::parse(r#"[{},{"days":99999}]"#).is_err());
        assert!(Levels::parse(r#"[{},{"auto":false},{"auto":true}]"#).is_err(), "an earned level needs the one below earned");
        assert!(Levels::parse(r#"[{},{"name":"a<b"}]"#).is_err());
        assert!(Levels::parse(r#"[{},{"auto":true,"posts":3}]"#).is_ok());
        let too_many = format!("[{}]", vec!["{}"; 22].join(","));
        assert!(Levels::parse(&too_many).is_err());
    }

    #[test]
    fn progress_percent() {
        let r = Requirement { kind: Kind::Posts, needed: 50, current: 12 };
        assert_eq!(r.percent(), 24);
        assert!(!r.met());
        assert_eq!(Requirement { kind: Kind::Posts, needed: 5, current: 9 }.percent(), 100);
    }

    #[test]
    fn climbs_while_met_and_never_lowers() {
        let s = settings(&[
            ("trust_level_1_posts", 5),
            ("trust_level_2_posts", 50),
            ("trust_level_2_days_visited", 10),
            ("trust_level_3_posts", 200),
        ]);
        let m = Metrics { posts: 60, days_visited: 3, ..Metrics::default() };
        assert_eq!(earned_level(&s, 0, &m), 1, "level 2 also needs ten days of visits");
        let m = Metrics { posts: 60, days_visited: 10, ..Metrics::default() };
        assert_eq!(earned_level(&s, 0, &m), 2);
        assert_eq!(earned_level(&s, 3, &Metrics::default()), 3);
    }

    #[test]
    fn a_new_level_is_climbed_to_and_a_manual_one_is_not() {
        let mut l = Levels::default();
        l.0.push(Level { level: 5, auto: true, posts: 1000, ..Level::default() });
        // Level 4 is given by hand, so earning stops at 3 even though 5 is marked earned.
        let get = |k: &str| l.value(k);
        assert_eq!(max_auto(&get), 3);
        l.0[4].auto = true;
        let get = |k: &str| l.value(k);
        assert_eq!(max_auto(&get), 5);
        let rich = Metrics { posts: 5000, days: 999, topics_read: 999, likes_received: 99, likes_given: 99, days_visited: 99, reputation: 99, ..Metrics::default() };
        assert_eq!(earned_level(&get, 0, &rich), 5);
    }

    #[test]
    fn nearly_there_needs_every_requirement_close() {
        let s = settings(&[("trust_level_1_posts", 10), ("trust_level_1_likes_received", 10)]);
        let close = requirements(&s, 1, &Metrics { posts: 9, likes_received: 8, ..Metrics::default() });
        assert!(nearly_there(&close));
        let far = requirements(&s, 1, &Metrics { posts: 9, likes_received: 2, ..Metrics::default() });
        assert!(!nearly_there(&far));
        let done = requirements(&s, 1, &Metrics { posts: 10, likes_received: 10, ..Metrics::default() });
        assert!(!nearly_there(&done), "met is not nearly");
    }

    #[test]
    fn a_level_is_kept_by_recent_posts() {
        let s = settings(&[("trust_level_3_recent_posts", 3), ("trust_level_2_recent_posts", 0)]);
        assert!(keeps_level(&s, 3, &Metrics { recent_posts: 3, ..Metrics::default() }));
        assert!(!keeps_level(&s, 3, &Metrics { recent_posts: 2, ..Metrics::default() }));
        assert!(keeps_level(&s, 2, &Metrics::default()), "no requirement keeps it");
    }
}
