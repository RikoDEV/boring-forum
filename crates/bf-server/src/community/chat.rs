//! Built-in chat (public channels, direct messages, reactions, replies, pins, flags), and rules
//! that send forum activity to outside chat services.

use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Router;
use axum::routing::{delete, get, post, put};
use bf_core::AppError;
use bf_db::community::chat;

mod admin;
mod channels;
mod invite_only;
mod membership;
mod messages;
mod rules;
mod send;

use self::admin::{
    add_channel, admin_channels, admin_flags, bad_node, decide_flag, delete_channel, update_channel,
};
use self::channels::{chat_channel_page, chat_page, list_channels, me, messages, open_dm};
use self::invite_only::{add_member, list_members, member_channel, remove_member};
use self::membership::{join, leave, mute, read};
use self::messages::{changed, edit, flag, pin, react, remove, staff_decide};
use self::rules::{add_rule, delete_rule, list_rules, test_rule, update_rule};
use self::send::{check_length, screen, send};

pub(crate) use self::rules::{integrate, run_retention};
pub(crate) use self::send::{check_can_deliver, deliver_edit, deliver_message};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chat", get(chat_page))
        .route("/chat/{id}", get(chat_channel_page))
        .route("/api/v1/chat/channels", get(list_channels).post(member_channel))
        .route("/api/v1/chat/channels/{id}/members", get(list_members).post(add_member))
        .route("/api/v1/chat/channels/{id}/members/{user}", delete(remove_member))
        .route("/api/v1/chat/me", get(me))
        .route("/chat/flags/{id}", post(staff_decide))
        .route("/api/v1/chat/dm", post(open_dm))
        .route("/api/v1/chat/channels/{id}/messages", get(messages).post(send))
        .route("/api/v1/chat/channels/{id}/join", post(join))
        .route("/api/v1/chat/channels/{id}/leave", post(leave))
        .route("/api/v1/chat/channels/{id}/mute", post(mute))
        .route("/api/v1/chat/channels/{id}/read", post(read))
        .route("/api/v1/chat/messages/{id}", put(edit).delete(remove))
        .route("/api/v1/chat/messages/{id}/react", post(react))
        .route("/api/v1/chat/messages/{id}/pin", post(pin))
        .route("/api/v1/chat/messages/{id}/flag", post(flag))
        .route("/api/v1/admin/chat/channels", get(admin_channels).post(add_channel))
        .route("/api/v1/admin/chat/channels/{id}", put(update_channel).delete(delete_channel))
        .route("/api/v1/admin/chat/flags", get(admin_flags))
        .route("/api/v1/admin/chat/flags/{id}", delete(decide_flag))
        .route("/api/v1/admin/chat/flags/{id}/remove", post(decide_flag))
        .route("/api/v1/admin/chat/rules", get(list_rules).post(add_rule))
        .route("/api/v1/admin/chat/rules/{id}", put(update_rule).delete(delete_rule))
        .route("/api/v1/admin/chat/rules/{id}/test", post(test_rule))
}

pub const PROVIDERS: [&str; 10] =
    ["slack", "discord", "mattermost", "rocketchat", "teams", "googlechat", "webex", "telegram", "matrix", "zulip"];

fn enabled(state: &AppState) -> Result<(), AppError> {
    state.require_feature("chat_enabled")
}

/// The channel, if this viewer may see it. Unknown and forbidden look the same.
pub(crate) async fn viewable(state: &AppState, v: &Viewer, id: i64) -> Result<chat::Channel, AppError> {
    enabled(state)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let c = chat::channel(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    // Invite-only channels stay private from staff too, like direct messages.
    let allowed = match (c.kind.as_str(), c.access.as_str()) {
        ("dm", _) | (_, "invite") => chat::is_member(pool, id, user.id.get()).await.db()?,
        (_, "groups") => {
            // Current groups decide: having joined once is no pass after the rules change.
            v.actor.is_admin
                || chat::group_allowed(pool, id, &bf_core::ids::raw(&v.actor.groups)).await.db()?
        }
        _ => true,
    };
    if allowed { Ok(c) } else { Err(AppError::NotFound) }
}

/// The message and channel, if the viewer can see them.
async fn visible_message(state: &AppState, v: &Viewer, id: i64) -> Result<(chat::Message, chat::Channel), AppError> {
    let m = chat::message(state.db()?, id).await.db()?.ok_or(AppError::NotFound)?;
    let c = viewable(state, v, m.channel_id).await?;
    Ok((m, c))
}

fn writable(v: &Viewer, c: &chat::Channel) -> Result<(), AppError> {
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    if c.archived {
        return Err(AppError::Validation("This channel is archived.".into()));
    }
    Ok(())
}
