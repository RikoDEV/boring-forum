//! Polls and votes: several per topic, choice, ranked-choice or rating, with an optional closing
//! time, public voters, final votes, and limits on who may vote.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;

type Error = sqlx::Error;

/// Most polls one topic may carry.
pub const MAX_PER_TOPIC: i64 = 5;

/// Most options a poll has; a rating poll's numbers count as options.
pub const MAX_OPTIONS: usize = 20;

#[derive(Debug, Clone, FromRow)]
pub struct Poll {
    pub id: i64,
    pub item_id: i64,
    pub question: String,
    pub max_choices: i32,
    pub closes_at: Option<DateTime<Utc>>,
    /// `always`, `voted` (after the viewer votes), `closed` (after the poll closes) or `staff`.
    pub results: String,
    pub voters: i64,
    /// `choice`, `ranked` (members order the options) or `rating` (pick a number).
    pub kind: String,
    /// Who voted for what is shown.
    pub is_public: bool,
    /// Members may change or withdraw their vote.
    pub vote_change: bool,
    pub min_trust_level: i16,
    /// The reply this poll was made in; None for the topic.
    pub post_number: Option<i32>,
    /// Group ids and names allowed to vote; empty means everyone.
    pub groups: Vec<i64>,
    pub group_names: Vec<String>,
}

/// `SELECT <the poll's columns> FROM polls p <tail>`, as one literal.
macro_rules! select_poll {
    ($tail:literal) => {
        concat!(
            "SELECT p.id, p.item_id, p.question, p.max_choices, p.closes_at, p.results,
    (SELECT count(DISTINCT v.user_id) FROM poll_votes v WHERE v.poll_id = p.id) AS voters,
    p.kind, p.is_public, p.vote_change, p.min_trust_level, p.post_number,
    ARRAY(SELECT g.id FROM poll_groups pg JOIN groups g ON g.id = pg.group_id WHERE pg.poll_id = p.id ORDER BY g.name) AS groups,
    ARRAY(SELECT g.name FROM poll_groups pg JOIN groups g ON g.id = pg.group_id WHERE pg.poll_id = p.id ORDER BY g.name) AS group_names
    FROM polls p ",
            $tail
        )
    };
}

impl Poll {
    /// Closed by its time, or with its topic when that is locked or archived. `now` is the one
    /// clock for the request (read once by the caller); `topic_inactive` is locked or archived.
    pub fn closed(&self, topic_inactive: bool, now: DateTime<Utc>) -> bool {
        topic_inactive || self.closes_at.is_some_and(|c| c <= now)
    }

    /// Whether a member with these groups and trust level may vote.
    pub fn may_vote(&self, member_groups: &[i64], trust_level: i16) -> bool {
        trust_level >= self.min_trust_level
            && (self.groups.is_empty() || self.groups.iter().any(|g| member_groups.contains(g)))
    }

    /// Several answers at once: only a choice poll with room for more than one.
    pub fn multiple(&self) -> bool {
        self.kind == "choice" && self.max_choices > 1
    }
}

/// A new poll. `Default` is a single-choice poll anyone may vote in; set what differs.
#[derive(Default)]
pub struct NewPoll<'a> {
    pub question: &'a str,
    pub options: &'a [String],
    pub max_choices: i32,
    pub closes_at: Option<DateTime<Utc>>,
    /// `always` (also the empty default), `voted`, `closed` or `staff`.
    pub results: &'a str,
    /// `choice` (also the empty default), `ranked` or `rating`.
    pub kind: &'a str,
    pub is_public: bool,
    /// Votes cannot be changed once cast.
    pub final_votes: bool,
    pub min_trust_level: i16,
    pub post_number: Option<i32>,
    pub groups: &'a [i64],
}

/// The values of a rating poll, from `min` to `max` in steps of `step`; None when that is not
/// two to `MAX_OPTIONS` numbers.
pub fn rating_options(min: i32, max: i32, step: i32) -> Option<Vec<String>> {
    if step < 1 || max <= min {
        return None;
    }
    let count = ((i64::from(max) - i64::from(min)) / i64::from(step)) as usize + 1;
    if !(2..=MAX_OPTIONS).contains(&count) {
        return None;
    }
    Some((0..count as i32).map(|i| (min + i * step).to_string()).collect())
}

/// A wall-clock time in a named zone as a moment. None for a zone Postgres does not know.
pub async fn local_to_utc(
    pool: &PgPool,
    local: chrono::NaiveDateTime,
    zone: &str,
) -> Result<Option<DateTime<Utc>>, Error> {
    sqlx::query_scalar("SELECT ($1::timestamp AT TIME ZONE n.name) FROM pg_timezone_names n WHERE n.name = $2 LIMIT 1")
        .bind(local)
        .bind(zone)
        .fetch_optional(pool)
        .await
}

/// Adds a poll to a topic. None when the topic already has `MAX_PER_TOPIC`.
pub async fn add_poll(pool: &PgPool, item: i64, p: &NewPoll<'_>) -> Result<Option<i64>, Error> {
    let mut tx = pool.begin().await?;
    // Serialises adds on one topic, so two at once cannot pass the limit.
    sqlx::query("SELECT id FROM items WHERE id = $1 FOR UPDATE").bind(item).execute(&mut *tx).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM polls WHERE item_id = $1").bind(item).fetch_one(&mut *tx).await?;
    if count >= MAX_PER_TOPIC {
        return Ok(None);
    }
    let kind = if p.kind.is_empty() { "choice" } else { p.kind };
    let results = if p.results.is_empty() { "always" } else { p.results };
    // Only a choice poll takes more than one answer; a ranked ballot may order every option.
    let max_choices = match kind {
        "ranked" => p.options.len() as i32,
        "choice" => p.max_choices.clamp(1, p.options.len().max(1) as i32),
        _ => 1,
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO polls (item_id, position, question, max_choices, closes_at, results, kind, is_public,
                            vote_change, min_trust_level, post_number)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id",
    )
    .bind(item)
    .bind(count as i32)
    .bind(p.question)
    .bind(max_choices)
    .bind(p.closes_at)
    .bind(results)
    .bind(kind)
    .bind(p.is_public)
    .bind(!p.final_votes)
    .bind(p.min_trust_level)
    .bind(p.post_number)
    .fetch_one(&mut *tx)
    .await?;
    set_groups(&mut tx, id, p.groups).await?;
    for (i, o) in p.options.iter().enumerate() {
        sqlx::query("INSERT INTO poll_options (item_id, poll_id, position, label) VALUES ($1, $2, $3, $4)")
            .bind(item)
            .bind(id)
            .bind(i as i32)
            .bind(o)
            .execute(&mut *tx)
            .await?;
    }
    sync_flag(&mut tx, item).await?;
    tx.commit().await?;
    Ok(Some(id))
}

async fn set_groups(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, poll: i64, groups: &[i64]) -> Result<(), Error> {
    sqlx::query("DELETE FROM poll_groups WHERE poll_id = $1").bind(poll).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO poll_groups (poll_id, group_id) SELECT $1, g FROM unnest($2::bigint[]) AS g ON CONFLICT DO NOTHING")
        .bind(poll)
        .bind(groups)
        .execute(&mut **tx)
        .await
        .map(|_| ())
}

/// The ids of the groups with these names (any case), and the names that match none.
pub async fn groups_by_name(pool: &PgPool, names: &[String]) -> Result<(Vec<i64>, Vec<String>), Error> {
    let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let found: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, lower(name) FROM groups WHERE lower(name) = ANY($1) AND id <> 1")
            .bind(&lower)
            .fetch_all(pool)
            .await?;
    let unknown = names.iter().filter(|n| !found.iter().any(|(_, f)| *f == n.to_lowercase())).cloned().collect();
    Ok((found.into_iter().map(|(id, _)| id).collect(), unknown))
}

/// The author of one post of a topic, for who may attach a poll to it. None: no such post.
pub async fn post_author(pool: &PgPool, item: i64, number: i32) -> Result<Option<Option<i64>>, Error> {
    sqlx::query_scalar("SELECT author_id FROM posts WHERE item_id = $1 AND post_number = $2")
        .bind(item)
        .bind(number)
        .fetch_optional(pool)
        .await
}

/// Removes one poll and its votes. False when it is not on that topic.
pub async fn delete_poll(pool: &PgPool, item: i64, poll: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let n = sqlx::query("DELETE FROM polls WHERE id = $1 AND item_id = $2")
        .bind(poll)
        .bind(item)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sync_flag(&mut tx, item).await?;
    tx.commit().await?;
    Ok(n > 0)
}

/// `items.poll_question` is the first poll's question, or NULL without polls.
async fn sync_flag(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, item: i64) -> Result<(), Error> {
    sqlx::query(
        "UPDATE items SET poll_question =
           (SELECT question FROM polls WHERE item_id = $1 ORDER BY position, id LIMIT 1) WHERE id = $1",
    )
    .bind(item)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

pub async fn polls_of(pool: &PgPool, item: i64) -> Result<Vec<Poll>, Error> {
    sqlx::query_as(select_poll!("WHERE p.item_id = $1 ORDER BY p.position, p.id"))
        .bind(item)
        .fetch_all(pool)
        .await
}

pub async fn poll(pool: &PgPool, item: i64, poll: i64) -> Result<Option<Poll>, Error> {
    sqlx::query_as(select_poll!("WHERE p.id = $1 AND p.item_id = $2"))
        .bind(poll)
        .bind(item)
        .fetch_optional(pool)
        .await
}

/// (poll id, option id, label, votes, whether this viewer chose it), for every poll of a topic.
pub async fn options_of(
    pool: &PgPool,
    item: i64,
    viewer: Option<i64>,
) -> Result<Vec<(i64, i64, String, i64, bool)>, Error> {
    sqlx::query_as(
        "SELECT o.poll_id, o.id, o.label, (SELECT count(*) FROM poll_votes v WHERE v.option_id = o.id),
                EXISTS (SELECT 1 FROM poll_votes v WHERE v.option_id = o.id AND v.user_id = $2)
         FROM poll_options o WHERE o.item_id = $1 ORDER BY o.poll_id, o.position",
    )
    .bind(item)
    .bind(viewer)
    .fetch_all(pool)
    .await
}

/// Whether this member has a vote in the poll.
pub async fn has_voted(pool: &PgPool, poll: i64, user: i64) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM poll_votes WHERE poll_id = $1 AND user_id = $2)")
        .bind(poll)
        .bind(user)
        .fetch_one(pool)
        .await
}

/// Replaces this member's choices in one poll; for a ranked poll `options` are in the order of
/// their ranking, best first. None when an option is not in the poll, one is repeated, there are
/// more than it allows, or the vote is final and already cast. Returns whether they had voted
/// before.
pub async fn vote(pool: &PgPool, poll: &Poll, options: &[i64], user: i64) -> Result<Option<bool>, Error> {
    if options.is_empty() || options.len() > poll.max_choices.max(1) as usize {
        return Ok(None);
    }
    let mut tx = pool.begin().await?;
    // One ballot at a time per member and poll: two requests at once would both pass the checks
    // below and both be counted.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('poll_vote:' || $1::text || ':' || $2::text))")
        .bind(poll.id)
        .bind(user)
        .execute(&mut *tx)
        .await?;
    let valid: i64 = sqlx::query_scalar("SELECT count(*) FROM poll_options WHERE poll_id = $1 AND id = ANY($2)")
        .bind(poll.id)
        .bind(options)
        .fetch_one(&mut *tx)
        .await?;
    let distinct = options.iter().collect::<std::collections::HashSet<_>>().len();
    if valid != options.len() as i64 || distinct != options.len() {
        return Ok(None);
    }
    if !poll.vote_change {
        let voted: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM poll_votes WHERE poll_id = $1 AND user_id = $2)")
                .bind(poll.id)
                .bind(user)
                .fetch_one(&mut *tx)
                .await?;
        if voted {
            return Ok(None);
        }
    }
    let before = sqlx::query("DELETE FROM poll_votes WHERE poll_id = $1 AND user_id = $2")
        .bind(poll.id)
        .bind(user)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        > 0;
    sqlx::query(
        "INSERT INTO poll_votes (item_id, poll_id, user_id, option_id, rank)
         SELECT $1, $2, $3, t.o, CASE WHEN $5 THEN t.ord::smallint END
         FROM unnest($4::bigint[]) WITH ORDINALITY AS t(o, ord)
         ON CONFLICT DO NOTHING",
    )
    .bind(poll.item_id)
    .bind(poll.id)
    .bind(user)
    .bind(options)
    .bind(poll.kind == "ranked")
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(before))
}

/// Withdraws this member's vote in one poll.
pub async fn unvote(pool: &PgPool, poll: i64, user: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM poll_votes WHERE poll_id = $1 AND user_id = $2")
        .bind(poll)
        .bind(user)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Every ballot of one poll: each member's options, best first (an unranked poll's in option order).
pub async fn ballots(pool: &PgPool, poll: i64) -> Result<Vec<Vec<i64>>, Error> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT v.user_id, v.option_id FROM poll_votes v JOIN poll_options o ON o.id = v.option_id
         WHERE v.poll_id = $1 ORDER BY v.user_id, v.rank NULLS LAST, o.position",
    )
    .bind(poll)
    .fetch_all(pool)
    .await?;
    let mut out: Vec<Vec<i64>> = Vec::new();
    let mut last = None;
    for (user, option) in rows {
        if last != Some(user) {
            out.push(Vec::new());
            last = Some(user);
        }
        if let Some(b) = out.last_mut() {
            b.push(option);
        }
    }
    Ok(out)
}

/// Who chose each option of the public polls of a topic: (poll, option, username), by name, at
/// most `limit` rows per poll.
pub async fn voters_of(pool: &PgPool, item: i64, limit: i64) -> Result<Vec<(i64, i64, String)>, Error> {
    sqlx::query_as(
        "SELECT poll_id, option_id, username FROM (
           SELECT v.poll_id, v.option_id, u.username, u.username_lower,
                  row_number() OVER (PARTITION BY v.poll_id ORDER BY u.username_lower, v.option_id) AS n
           FROM poll_votes v JOIN polls p ON p.id = v.poll_id JOIN users u ON u.id = v.user_id
           WHERE v.item_id = $1 AND p.is_public) x
         WHERE n <= $2 ORDER BY poll_id, option_id, username_lower",
    )
    .bind(item)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// What the author or a moderator may change in a poll.
pub struct Edit<'a> {
    pub question: &'a str,
    /// The options in order. Ignored for a rating poll, whose numbers are fixed.
    pub options: &'a [String],
    pub max_choices: i32,
    pub closes_at: Option<DateTime<Utc>>,
    pub results: &'a str,
    pub is_public: bool,
    pub final_votes: bool,
    pub min_trust_level: i16,
    pub groups: &'a [i64],
    /// Allow dropping options that already have votes.
    pub confirm_removal: bool,
}

pub enum Edited {
    Saved,
    NotFound,
    /// Votes the edit would delete along with options removed from the end of the list.
    WouldRemoveVotes(i64),
}

/// Changes a poll in place. Options keep their votes by position: the first line is the first
/// option, renamed if its text changed. Lines added go on the end; lines removed come off it.
pub async fn edit_poll(pool: &PgPool, item: i64, poll: i64, e: &Edit<'_>) -> Result<Edited, Error> {
    let mut tx = pool.begin().await?;
    let Some(kind): Option<String> =
        sqlx::query_scalar("SELECT kind FROM polls WHERE id = $1 AND item_id = $2 FOR UPDATE")
            .bind(poll)
            .bind(item)
            .fetch_optional(&mut *tx)
            .await?
    else {
        return Ok(Edited::NotFound);
    };
    if kind != "rating" {
        let existing: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, label FROM poll_options WHERE poll_id = $1 ORDER BY position, id")
                .bind(poll)
                .fetch_all(&mut *tx)
                .await?;
        let keep = existing.len().min(e.options.len());
        let dropped: Vec<i64> = existing[keep..].iter().map(|(id, _)| *id).collect();
        if !dropped.is_empty() {
            let votes: i64 = sqlx::query_scalar("SELECT count(*) FROM poll_votes WHERE option_id = ANY($1)")
                .bind(&dropped)
                .fetch_one(&mut *tx)
                .await?;
            if votes > 0 && !e.confirm_removal {
                return Ok(Edited::WouldRemoveVotes(votes));
            }
            sqlx::query("DELETE FROM poll_options WHERE id = ANY($1)").bind(&dropped).execute(&mut *tx).await?;
        }
        for (i, label) in e.options.iter().enumerate() {
            match existing.get(i) {
                Some((id, old)) if old != label => {
                    sqlx::query("UPDATE poll_options SET label = $2 WHERE id = $1")
                        .bind(id)
                        .bind(label)
                        .execute(&mut *tx)
                        .await?;
                }
                Some(_) => {}
                None => {
                    sqlx::query("INSERT INTO poll_options (item_id, poll_id, position, label) VALUES ($1, $2, $3, $4)")
                        .bind(item)
                        .bind(poll)
                        .bind(i as i32)
                        .bind(label)
                        .execute(&mut *tx)
                        .await?;
                }
            }
        }
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM poll_options WHERE poll_id = $1")
        .bind(poll)
        .fetch_one(&mut *tx)
        .await?;
    let max_choices = match kind.as_str() {
        "ranked" => count as i32,
        "choice" => e.max_choices.clamp(1, count.max(1) as i32),
        _ => 1,
    };
    sqlx::query(
        "UPDATE polls SET question = $2, max_choices = $3, closes_at = $4, results = $5, is_public = $6,
                vote_change = $7, min_trust_level = $8,
                closed_notified_at = CASE WHEN $4::timestamptz IS NOT DISTINCT FROM closes_at THEN closed_notified_at ELSE NULL END
         WHERE id = $1",
    )
    .bind(poll)
    .bind(e.question)
    .bind(max_choices)
    .bind(e.closes_at)
    .bind(e.results)
    .bind(e.is_public)
    .bind(!e.final_votes)
    .bind(e.min_trust_level)
    .execute(&mut *tx)
    .await?;
    set_groups(&mut tx, poll, e.groups).await?;
    sync_flag(&mut tx, item).await?;
    tx.commit().await?;
    Ok(Edited::Saved)
}

/// Closes a poll now, or opens it again (no closing time). False when it is not on that topic.
pub async fn set_closed(pool: &PgPool, item: i64, poll: i64, closed: bool) -> Result<bool, Error> {
    let n = sqlx::query(
        "UPDATE polls SET closes_at = CASE WHEN $3 THEN now() ELSE NULL END,
                closed_notified_at = CASE WHEN $3 THEN closed_notified_at ELSE NULL END
         WHERE id = $1 AND item_id = $2",
    )
    .bind(poll)
    .bind(item)
    .bind(closed)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// The outcome of counting ranked ballots by instant runoff.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Runoff {
    /// Options, winner first, then the rest by how late they went out.
    pub order: Vec<i64>,
    /// First choices each option got.
    pub first_choices: HashMap<i64, i64>,
    /// Votes the winner held in the last round, and how many ballots were still counted then.
    pub winner_votes: i64,
    pub counted: i64,
    pub rounds: usize,
}

/// Instant-runoff count: each ballot counts for its best option still in the race; the option
/// with the fewest votes goes out each round (ties: the one listed last) until one holds more
/// than half of the ballots that still name a candidate. `options` are in display order.
pub fn instant_runoff(ballots: &[Vec<i64>], options: &[i64]) -> Runoff {
    let mut out = Runoff::default();
    let mut remaining: Vec<i64> = options.to_vec();
    let mut eliminated: Vec<i64> = Vec::new();
    let place = |o: &i64| options.iter().position(|x| x == o).unwrap_or(usize::MAX);
    loop {
        let mut tally: HashMap<i64, i64> = remaining.iter().map(|o| (*o, 0)).collect();
        let mut counted = 0;
        for b in ballots {
            if let Some(top) = b.iter().find(|o| remaining.contains(o)) {
                *tally.entry(*top).or_default() += 1;
                counted += 1;
            }
        }
        out.rounds += 1;
        if out.rounds == 1 {
            out.first_choices = tally.clone();
        }
        // The most votes; of equals the one listed first.
        let best = remaining.iter().copied().max_by_key(|o| (tally[o], std::cmp::Reverse(place(o))));
        if remaining.len() <= 1 || best.is_some_and(|b| tally[&b] * 2 > counted) {
            out.winner_votes = best.map_or(0, |b| tally[&b]);
            out.counted = counted;
            let mut rest: Vec<i64> = remaining.iter().copied().filter(|o| Some(*o) != best).collect();
            rest.sort_by_key(|o| (std::cmp::Reverse(tally[o]), place(o)));
            out.order = best.into_iter().chain(rest).chain(eliminated.iter().rev().copied()).collect();
            return out;
        }
        // Fewest votes goes out; of equals the one listed last.
        let Some(lowest) = remaining.iter().copied().min_by_key(|o| (tally[o], std::cmp::Reverse(place(o)))) else {
            return out;
        };
        remaining.retain(|o| *o != lowest);
        eliminated.push(lowest);
    }
}

/// Polls closed by their time or by their topic being locked, each returned once:
/// (topic, slug, title, forum, voters and the topic's author).
pub async fn due_poll_closures(pool: &PgPool) -> Result<Vec<(i64, String, String, i64, Vec<i64>)>, Error> {
    sqlx::query_as(
        "WITH due AS (
           UPDATE polls p SET closed_notified_at = now()
           FROM items i WHERE i.id = p.item_id AND p.closed_notified_at IS NULL
             AND (i.locked OR i.archived OR p.closes_at <= now())
           RETURNING p.id, p.item_id)
         SELECT i.id, i.slug, i.title, i.node_id,
                ARRAY(SELECT user_id FROM poll_votes WHERE poll_id = due.id
                      UNION SELECT author_id FROM posts WHERE item_id = i.id AND post_number = 1 AND author_id IS NOT NULL)
         FROM due JOIN items i ON i.id = due.item_id WHERE i.state = 'visible'",
    )
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rating_values() {
        assert_eq!(rating_options(1, 5, 1).unwrap(), ["1", "2", "3", "4", "5"]);
        assert_eq!(rating_options(0, 10, 5).unwrap(), ["0", "5", "10"]);
        assert!(rating_options(5, 5, 1).is_none(), "needs two values");
        assert!(rating_options(1, 100, 1).is_none(), "too many values");
        assert!(rating_options(1, 5, 0).is_none());
    }

    #[test]
    fn runoff_picks_a_majority_winner_in_the_first_round() {
        let ballots = vec![vec![1, 2], vec![1], vec![1, 3], vec![2], vec![3]];
        let r = instant_runoff(&ballots, &[1, 2, 3]);
        assert_eq!((r.order[0], r.winner_votes, r.counted, r.rounds), (1, 3, 5, 1));
    }

    #[test]
    fn runoff_moves_the_votes_of_the_eliminated() {
        // 1 and 2 have 3 first choices each, 3 has 2: 3 goes out and their ballots go to 2, who wins 5 to 3.
        let ballots = vec![vec![1], vec![1], vec![1], vec![2], vec![2], vec![2], vec![3, 2], vec![3, 2]];
        let r = instant_runoff(&ballots, &[1, 2, 3]);
        assert_eq!(r.order, [2, 1, 3]);
        assert_eq!((r.winner_votes, r.counted, r.rounds), (5, 8, 2));
        assert_eq!(r.first_choices[&3], 2);
    }

    #[test]
    fn runoff_without_ballots_names_the_first_option() {
        let r = instant_runoff(&[], &[7, 8]);
        assert_eq!(r.winner_votes, 0);
        assert_eq!(r.order.len(), 2);
    }
}
