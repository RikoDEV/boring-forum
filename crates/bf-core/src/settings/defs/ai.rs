//! AI features: the model provider and each feature on top of it. Everything is off by default.

use super::super::{SettingDef, SettingType::*};
use super::{def, int};
use crate::i18n::en;
use serde_json::json;

fn flag(key: &'static str, on: bool, public: bool, description: &'static str) -> SettingDef {
    SettingDef { default: json!(on), ..def(key, Bool, public, description) }
}

fn text(key: &'static str, default: &str, description: &'static str) -> SettingDef {
    SettingDef { default: json!(default), ..def(key, String, false, description) }
}

pub(super) fn defs() -> Vec<SettingDef> {
    vec![
        // The provider.
        flag("ai_enabled", false, true, en("AI: turn on the features below (needs a provider)")),
        text("ai_api_style", "openai", en("AI provider: openai (OpenAI, Ollama, vLLM, OpenRouter, Gemini and others that speak its API), anthropic or mock (canned answers, no network; for development and tests)")),
        text("ai_base_url", "https://api.openai.com/v1", en("AI provider: base address of the API")),
        flag(
            "ai_allow_private_hosts",
            false,
            false,
            en("AI provider: allow an address on this server's own network (Ollama or vLLM on the same host); off refuses private and loopback addresses"),
        ),
        text("ai_api_key", "", en("AI provider: API key (BF_AI_API_KEY in the environment takes precedence)")),
        text("ai_model", "gpt-4o-mini", en("AI provider: model for summaries, moderation, translation and the bot")),
        text("ai_embedding_model", "", en("AI embeddings: model for related topics and semantic search; empty turns them off")),
        text("ai_embedding_base_url", "", en("AI embeddings: base address when it differs from the provider's; empty uses the provider's")),
        text("ai_embedding_api_key", "", en("AI embeddings: API key when it differs from the provider's (BF_AI_EMBEDDING_API_KEY takes precedence)")),
        SettingDef {
            default: json!(0),
            min: Some(0),
            max: Some(4096),
            ..def("ai_embedding_dims", Int, false, en("AI embeddings: vector size to ask for; 0 keeps the model's own"))
        },
        flag(
            "ai_public_only",
            true,
            false,
            en("AI: only read content that guests can see; private forums and messages never reach the provider"),
        ),
        SettingDef {
            default: json!(0),
            min: Some(0),
            max: Some(2_000_000_000),
            ..def("ai_monthly_token_limit", Int, false, en("AI: tokens the whole site may use per month; 0 for no limit"))
        },
        int("ai_user_daily_limit", 30, 0, 100_000, en("AI: requests one member may make per day (summaries, translations, helper, bot); 0 for no limit")),
        // Summaries.
        flag("ai_summaries_enabled", false, true, en("AI: members can have a topic summarized")),
        int("ai_summary_min_replies", 5, 1, 1000, en("AI: shortest topic (in replies) that offers a summary")),
        flag("ai_gists_enabled", false, true, en("AI: busy topics get a one-line gist on their preview card")),
        // Moderation.
        flag("ai_spam_enabled", false, false, en("AI: judge new posts of newer members for spam")),
        text("ai_spam_action", "flag", en("AI spam: what to do with a post judged spam: flag (to the review queue), hold (hide until approved) or hide")),
        int("ai_spam_threshold", 80, 1, 100, en("AI spam: score from 1 to 100 at which a post counts as spam")),
        int("ai_max_trust_level", 1, 0, 20, en("AI moderation: only posts by members up to this trust level are judged")),
        flag("ai_toxicity_enabled", false, false, en("AI: flag abusive or hateful posts for review")),
        int("ai_toxicity_threshold", 85, 1, 100, en("AI toxicity: score from 1 to 100 at which a post is flagged")),
        flag("ai_sentiment_enabled", false, false, en("AI: record the mood of each new post for the admin report")),
        // Translation.
        flag("ai_translate_enabled", false, true, en("AI: readers can have a post translated into their language")),
        text("ai_auto_translate_locales", "", en("AI: languages (codes such as pl,ar) to translate every new topic into at once; empty for none")),
        // Search and related topics.
        flag("ai_semantic_search_enabled", false, false, en("AI: search also finds topics by meaning (needs an embedding model)")),
        flag("ai_related_topics_enabled", false, true, en("AI: topics show others about the same thing (needs an embedding model)")),
        int("ai_embedding_max_topics", 50_000, 100, 500_000, en("AI embeddings: most topics kept in memory for similarity search")),
        // Composer helper and bot.
        flag("ai_helper_enabled", false, true, en("AI: the composer offers proofreading, tone, titles and tags")),
        int("ai_helper_min_trust_level", 0, 0, 20, en("AI helper: lowest trust level that may use it")),
        flag("ai_bot_enabled", false, false, en("AI: personas answer when they are mentioned in a topic")),
    ]
}
