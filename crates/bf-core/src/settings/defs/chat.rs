//! Chat limits and retention.

use super::super::{SettingDef, SettingType::*};
use super::{def, int};
use crate::i18n::en;
use serde_json::json;

pub(super) fn defs() -> Vec<SettingDef> {
    vec![
        SettingDef { default: json!(true), ..def("chat_enabled", Bool, true, en("Let members use chat")) },
        int("chat_max_message_length", 6000, 1, 50_000, en("Longest chat message, in characters")),
        int("chat_max_mentions", 5, 0, 100, en("Only this many mentions in a chat message send notifications")),
        int("chat_max_dm_users", 20, 1, 100, en("Most people in a direct message")),
        int("chat_messages_per_minute", 30, 1, 1000, en("Chat messages one member can send per minute")),
        int("chat_min_trust_level", 0, 0, 20, en("Trust level needed to write in chat channels")),
        int("chat_create_min_trust_level", 2, 0, 20, en("Trust level needed to start an invite-only channel")),
        int("chat_dm_min_trust_level", 1, 0, 20, en("Trust level needed to start a direct message")),
        SettingDef {
            default: json!(true),
            ..def("chat_allow_uploads", Bool, true, en("Let members add files to chat messages"))
        },
        SettingDef {
            default: json!("png,jpg,gif,webp"),
            ..def(
                "chat_allowed_types",
                String,
                false,
                en("Kinds of file chat messages may carry; only those the site allows for uploads count"),
            )
        },
        int(
            "chat_upload_min_trust_level",
            1,
            0,
            20,
            en("Trust level needed to add files to chat messages; staff always may"),
        ),
        int("chat_max_files_per_message", 4, 1, 20, en("Most files one chat message may carry")),
        int("chat_wait_seconds", 0, 0, 3600, en("Minimum seconds between two chat messages by a member; 0 is off")),
        int(
            "chat_duplicate_seconds",
            60,
            0,
            86_400,
            en("Refuse the same message again within this many seconds; 0 is off"),
        ),
        int(
            "chat_auto_silence_hidden",
            3,
            0,
            100,
            en("Silence a member after this many messages hidden by flags in a day; 0 is off"),
        ),
        int("chat_silence_hours", 24, 1, 8760, en("How long that automatic silence lasts, in hours")),
        int("chat_retention_days", 0, 0, 36_500, en("Delete channel messages after this many days; 0 keeps them")),
        int("chat_dm_retention_days", 0, 0, 36_500, en("Delete direct messages after this many days; 0 keeps them")),
    ]
}
