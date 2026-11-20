//! One gate for what members write outside forum posts: private messages, statuses, profiles,
//! signatures, topic titles and usernames. Forum posts go through `forum::posting::screen`,
//! which uses the same account checks, so every place a member writes follows the same rules.

use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use bf_core::AppError;
use bf_core::watched::{Action, Outcome, Rule};
use bf_db::accounts::users::User;

/// The account itself may write: approved, 2FA set up where required, current terms accepted
/// and, where the site asks for it, a verified email.
pub(crate) fn account_ready(state: &AppState, user: &User) -> Result<(), String> {
    if !user.approved {
        return Err("Your account is waiting for approval by the staff.".into());
    }
    if user.needs_2fa {
        return Err("Your group requires two-factor authentication. Set it up under Account settings first.".into());
    }
    if user.legal_pending {
        return Err("Please review and accept the updated terms first: /legal/accept".into());
    }
    if state.settings().bool("require_email_verification") && !user.email_verified {
        return Err("Please verify your email address before posting (see Account settings).".into());
    }
    Ok(())
}

/// Text after watched words: what to store, and whether a "require approval" word means it
/// must wait in the held queue (`moderation::held`) instead of being written now.
pub(crate) struct Checked {
    pub text: String,
    pub held: bool,
}

/// Watched words for member text outside forum posts. Blocked words are refused; censor,
/// replace and link rules apply; a "require approval" word marks it held. Staff skip it, as
/// forum moderators do for posts.
pub(crate) async fn words(state: &AppState, v: &Viewer, text: &str) -> Result<Checked, AppError> {
    if is_staff(v) {
        return Ok(Checked { text: text.to_owned(), held: false });
    }
    let tree = state.forum.get(state.db()?).await?;
    match bf_core::watched::apply(&tree.words, text) {
        Outcome::Blocked(word) => {
            Err(AppError::invalid("That text contains a blocked word: {word}", &[("word", &word)]))
        }
        Outcome::Allow { text, needs_approval, .. } => Ok(Checked { text, held: needs_approval }),
    }
}

/// The new-member link limit (`new_user_posts`, `new_user_max_links`), for `links` links.
pub(crate) fn links(state: &AppState, v: &Viewer, user: &User, links: usize) -> Result<(), AppError> {
    let s = state.settings();
    if !is_staff(v)
        && i64::from(user.post_count) < s.int("new_user_posts")
        && links as i64 > s.int("new_user_max_links")
    {
        return Err(AppError::invalid(
            "New members can include at most {n} links here.",
            &[("n", &s.int("new_user_max_links"))],
        ));
    }
    Ok(())
}

/// The whole gate for one piece of member text: the account, watched words and the link limit.
/// Returns the text to store (and whether it must be held) with its cooked form.
pub(crate) async fn check(state: &AppState, v: &Viewer, text: &str) -> Result<(Checked, bf_cook::Cooked), AppError> {
    let user = v.require_user()?;
    account_ready(state, user).map_err(AppError::Validation)?;
    let checked = words(state, v, text).await?;
    let from = checked.text.clone();
    let cooked = tokio::task::spawn_blocking(move || bf_cook::cook(&from)).await.map_err(AppError::internal)?;
    links(state, v, user, cooked.links.len())?;
    Ok((checked, cooked))
}

/// `check` for text that is content (a post-like body: comments, messages, statuses):
/// the plugins' `post.before_create` / `post.before_cook` filters run first, so a refusal is the
/// member's error and whatever a plugin adds still meets the watched words and link limits.
pub(crate) async fn check_post(
    state: &AppState,
    v: &Viewer,
    text: &str,
) -> Result<(Checked, bf_cook::Cooked), AppError> {
    let user = v.require_user()?;
    let text = crate::platform::plugins::before_post(state, user.id.get(), text).await?;
    check(state, v, &text).await
}

/// May this be a username? Block, censor and require-approval words all rule it out; link,
/// replace and tag rules only make sense in running text.
pub(crate) fn username_ok(rules: &[Rule], name: &str) -> bool {
    let strict: Vec<Rule> = rules
        .iter()
        .filter(|r| matches!(r.action, Action::Block | Action::Censor | Action::RequireApproval | Action::Replace))
        .cloned()
        .collect();
    // Usernames join words with `_`, `-` and `.`; split them so `spam_bot` meets the `spam` rule.
    let spaced: String = name.chars().map(|c| if matches!(c, '_' | '-' | '.') { ' ' } else { c }).collect();
    matches!(bf_core::watched::apply(&strict, &spaced), Outcome::Allow { text, needs_approval: false, .. } if text == spaced)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(action: Action, pattern: &str) -> Rule {
        Rule { action, pattern: pattern.into(), replacement: Some("x".into()) }
    }

    #[test]
    fn usernames_meet_strict_rules_only() {
        let rules = [rule(Action::Block, "spam"), rule(Action::Censor, "darn"), rule(Action::Link, "rust")];
        assert!(!username_ok(&rules, "spam_bot"));
        assert!(!username_ok(&rules, "Darn"));
        assert!(username_ok(&rules, "rust"));
        assert!(username_ok(&rules, "spammer"));
    }
}
