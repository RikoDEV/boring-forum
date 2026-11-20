//! Talking to the model provider: chat completions and embeddings, with a budget, a per-member
//! daily limit and a usage log. Two API styles: OpenAI's (also spoken by Ollama, vLLM, OpenRouter,
//! Gemini's compatibility endpoint and others) and Anthropic's.

use crate::state::AppState;
use bf_core::AppError;
use bf_core::i18n::en;
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::Semaphore;

/// One request to the chat model.
pub struct Chat<'a> {
    /// What it is for (`summary`, `spam`, …): the usage log and the admin report group by it.
    pub feature: &'static str,
    /// The member who asked. `None` for background work, which only the monthly budget limits.
    pub user: Option<i64>,
    pub system: &'a str,
    /// ("user" or "assistant", text), oldest first; the last one is the user's.
    pub messages: Vec<(&'static str, String)>,
    pub max_tokens: u32,
    /// Overrides `ai_model`.
    pub model: Option<&'a str>,
    pub temperature: f32,
}

impl<'a> Chat<'a> {
    pub fn new(feature: &'static str, system: &'a str, prompt: String) -> Self {
        Self {
            feature,
            user: None,
            system,
            messages: vec![("user", prompt)],
            max_tokens: 600,
            model: None,
            temperature: 0.2,
        }
    }
    pub fn by(mut self, user: i64) -> Self {
        self.user = Some(user);
        self
    }
    pub fn max(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens;
        self
    }
}

pub struct Completion {
    pub text: String,
    pub model: String,
}

/// The offline provider for development and tests: `BF_AI_PROVIDER=mock` in the environment, or
/// `ai_api_style` set to `mock`. It answers from the prompt alone and never touches the network,
/// so the AI features can be exercised without an account or a key.
pub fn is_mock(state: &AppState) -> bool {
    std::env::var("BF_AI_PROVIDER").is_ok_and(|v| v.eq_ignore_ascii_case("mock"))
        || state.settings().str("ai_api_style") == "mock"
}

/// Whether the AI features can run: switched on, with an address to call (or the mock provider).
pub fn configured(state: &AppState) -> bool {
    let s = state.settings();
    s.bool("ai_enabled") && (is_mock(state) || !s.str("ai_base_url").trim().is_empty())
}

/// Whether embeddings can be made.
pub fn embeddings_configured(state: &AppState) -> bool {
    configured(state) && (is_mock(state) || !state.settings().str("ai_embedding_model").trim().is_empty())
}

/// A canned answer: JSON with every field the features read when the prompt asks for JSON, prose
/// otherwise. Deterministic, so tests can assert on it.
fn mock_answer(c: &Chat<'_>) -> String {
    let last = c.messages.last().map(|(_, t)| t.as_str()).unwrap_or_default();
    if c.system.contains("JSON") {
        json!({
            "spam": 0, "toxicity": 0, "sentiment": "neutral", "emotion": "neutral",
            "title": "", "text": clip(last, 4000), "titles": ["Mock title one", "Mock title two", "Mock title three"],
            "tags": [], "forum": 0, "mock": true,
        })
        .to_string()
    } else {
        format!("Mock answer: {}", clip(last, 200))
    }
}

/// A deterministic vector for a text (hashed words), so similar texts land near each other.
fn mock_vector(text: &str, dims: usize) -> Vec<f32> {
    use sha2::{Digest, Sha256};
    let mut v = vec![0.0_f32; dims];
    for word in text.split_whitespace().take(500) {
        let h = Sha256::digest(word.to_lowercase().as_bytes());
        let slot = (usize::from(h[0]) << 8 | usize::from(h[1])) % dims;
        v[slot] += if h[2] & 1 == 0 { 1.0 } else { -1.0 };
    }
    if v.iter().all(|x| *x == 0.0) {
        v[0] = 1.0;
    }
    v
}

/// Whether the provider may be on this server's own network.
fn private_allowed(state: &AppState) -> bool {
    state.settings().bool("ai_allow_private_hosts")
        || std::env::var("BF_AI_ALLOW_PRIVATE").is_ok_and(|v| v == "1")
        || crate::platform::outbound::loopback_allowed()
}

/// The HTTP client for one provider address. The address is set by an administrator, but a
/// careless or hijacked admin session must not turn this into a way to reach the host's own
/// network (cloud metadata, databases, the admin API). So unless `ai_allow_private_hosts` is on,
/// the name is resolved here, refused if any answer is private, loopback, link-local or otherwise
/// forbidden (the webhook list, `platform::outbound::forbidden_ip`), and the connection is pinned
/// to the addresses that were checked, so DNS cannot change between check and use. Redirects are
/// never followed. This does not go through `outbound::fetch_*` because those are GET helpers for
/// pages and files; the provider needs POST with a JSON body and a long timeout. A local Ollama
/// needs the setting.
async fn client_for(state: &AppState, root: &str) -> Result<reqwest::Client, AppError> {
    if private_allowed(state) {
        return Ok(http().clone());
    }
    let u = url::Url::parse(root).map_err(|_| unavailable())?;
    let host = u.host_str().ok_or_else(unavailable)?.to_owned();
    let port = u.port_or_known_default().unwrap_or(443);
    let addrs: Vec<std::net::SocketAddr> =
        tokio::net::lookup_host((host.as_str(), port)).await.map_err(|_| unavailable())?.collect();
    if addrs.is_empty() || addrs.iter().any(|a| crate::platform::outbound::forbidden_ip(a.ip())) {
        tracing::warn!(host = %host, "AI provider address is on a private network; turn on ai_allow_private_hosts to allow it");
        return Err(unavailable());
    }
    reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("boring-forum/", env!("CARGO_PKG_VERSION")))
        .resolve_to_addrs(&host, &addrs)
        .build()
        .map_err(|_| unavailable())
}

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("boring-forum/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

/// A few requests at a time, so a burst of posts cannot flood the provider (or the bill).
fn gate() -> &'static Semaphore {
    static GATE: OnceLock<Semaphore> = OnceLock::new();
    GATE.get_or_init(|| Semaphore::new(4))
}

fn key(state: &AppState, env: &str, setting: &str) -> String {
    std::env::var(env).ok().filter(|k| !k.is_empty()).unwrap_or_else(|| state.settings().str(setting).trim().to_owned())
}

/// A base address that is http(s), without credentials or a trailing slash.
fn base(raw: &str) -> Option<String> {
    let u = url::Url::parse(raw.trim()).ok()?;
    let ok = matches!(u.scheme(), "http" | "https")
        && u.host_str().is_some()
        && u.username().is_empty()
        && u.password().is_none();
    ok.then(|| u.as_str().trim_end_matches('/').to_owned())
}

fn unavailable() -> AppError {
    AppError::Validation(en("The AI service is not available right now.").into())
}

/// The monthly budget and, for a member, the daily limit.
pub async fn check_quota(state: &AppState, user: Option<i64>) -> Result<(), AppError> {
    let Some(pool) = &state.pool else { return Err(unavailable()) };
    let s = state.settings();
    let monthly = s.int("ai_monthly_token_limit");
    if monthly > 0 && bf_db::ai::tokens_this_month(pool).await.map_err(AppError::internal)? >= monthly {
        return Err(AppError::Validation(en("The site's AI allowance for this month is used up.").into()));
    }
    let daily = s.int("ai_user_daily_limit");
    if let Some(u) = user
        && daily > 0
        && bf_db::ai::requests_today(pool, u).await.map_err(AppError::internal)? >= daily
    {
        return Err(AppError::Validation(en("You have used today's AI requests. Try again tomorrow.").into()));
    }
    Ok(())
}

async fn record(state: &AppState, c: &Chat<'_>, model: &str, usage: (i32, i32), error: Option<&str>) {
    if let Some(pool) = &state.pool
        && let Err(e) =
            bf_db::ai::log_usage(pool, c.feature, model, c.user, usage.0, usage.1, error.is_none(), error).await
    {
        tracing::warn!(error = %e, "AI usage could not be logged");
    }
}

/// Newer OpenAI models take `max_completion_tokens` and a fixed temperature.
fn reasoning_model(model: &str) -> bool {
    ["o1", "o3", "o4", "gpt-5"].iter().any(|p| model.starts_with(p))
}

/// Asks the chat model. Failures are logged (with the provider's reason) and come back as a
/// short message; nothing about the request or the key leaves the server.
pub async fn chat(state: &AppState, c: Chat<'_>) -> Result<Completion, AppError> {
    if !configured(state) {
        return Err(unavailable());
    }
    check_quota(state, c.user).await?;
    let s = state.settings();
    if is_mock(state) {
        let text = mock_answer(&c);
        record(state, &c, "mock", (0, 0), None).await;
        return Ok(Completion { text, model: "mock".into() });
    }
    let model = c.model.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| s.str("ai_model")).trim().to_owned();
    let Some(root) = base(s.str("ai_base_url")) else { return Err(unavailable()) };
    let client = client_for(state, &root).await?;
    let anthropic = s.str("ai_api_style") == "anthropic";
    let api_key = key(state, "BF_AI_API_KEY", "ai_api_key");
    let _permit = gate().acquire().await.map_err(AppError::internal)?;
    let messages: Vec<Value> = c.messages.iter().map(|(role, text)| json!({ "role": role, "content": text })).collect();
    let (url, body) = if anthropic {
        (
            format!("{root}/messages"),
            json!({ "model": model, "max_tokens": c.max_tokens, "temperature": c.temperature, "system": c.system, "messages": messages }),
        )
    } else {
        let mut all = vec![json!({ "role": "system", "content": c.system })];
        all.extend(messages);
        let mut body = json!({ "model": model, "messages": all });
        if reasoning_model(&model) {
            body["max_completion_tokens"] = json!(c.max_tokens.max(1000));
        } else {
            body["max_tokens"] = json!(c.max_tokens);
            body["temperature"] = json!(c.temperature);
        }
        (format!("{root}/chat/completions"), body)
    };
    let mut req = client.post(url).json(&body);
    if anthropic {
        req = req.header("x-api-key", &api_key).header("anthropic-version", "2023-06-01");
    } else if !api_key.is_empty() {
        req = req.bearer_auth(&api_key);
    }
    let sent = req.send().await;
    let res = match sent {
        Ok(r) => r,
        Err(e) => {
            let why = e.without_url().to_string();
            tracing::warn!(feature = c.feature, error = %why, "AI provider could not be reached");
            record(state, &c, &model, (0, 0), Some(&why)).await;
            return Err(unavailable());
        }
    };
    let status = res.status();
    let value: Value = res.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let why = value.pointer("/error/message").and_then(Value::as_str).unwrap_or("no reason given");
        let why = format!("{} {why}", status.as_u16());
        tracing::warn!(feature = c.feature, error = %why, "AI provider refused a request");
        record(state, &c, &model, (0, 0), Some(&why)).await;
        return Err(unavailable());
    }
    let (text, usage) = if anthropic {
        let text: String = value["content"]
            .as_array()
            .map(|parts| parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(""))
            .unwrap_or_default();
        (
            text,
            (
                value["usage"]["input_tokens"].as_i64().unwrap_or(0),
                value["usage"]["output_tokens"].as_i64().unwrap_or(0),
            ),
        )
    } else {
        (
            value.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or_default().to_owned(),
            (
                value["usage"]["prompt_tokens"].as_i64().unwrap_or(0),
                value["usage"]["completion_tokens"].as_i64().unwrap_or(0),
            ),
        )
    };
    let usage = (usage.0 as i32, usage.1 as i32);
    if text.trim().is_empty() {
        record(state, &c, &model, usage, Some("empty answer")).await;
        return Err(unavailable());
    }
    record(state, &c, &model, usage, None).await;
    Ok(Completion { text: text.trim().to_owned(), model })
}

/// Vectors for some texts, each scaled to length 1 so that a dot product is the cosine.
pub async fn embed(
    state: &AppState,
    feature: &'static str,
    user: Option<i64>,
    inputs: &[String],
) -> Result<Vec<Vec<f32>>, AppError> {
    if !embeddings_configured(state) {
        return Err(unavailable());
    }
    let c = Chat { feature, user, system: "", messages: vec![], max_tokens: 0, model: None, temperature: 0.0 };
    check_quota(state, user).await?;
    let s = state.settings();
    if is_mock(state) {
        let dims = usize::try_from(s.int("ai_embedding_dims")).ok().filter(|d| *d > 0).unwrap_or(64);
        let out = inputs
            .iter()
            .map(|t| {
                let mut v = mock_vector(t, dims);
                let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                v.iter_mut().for_each(|x| *x /= norm);
                v
            })
            .collect();
        record(state, &c, "mock", (0, 0), None).await;
        return Ok(out);
    }
    let model = s.str("ai_embedding_model").trim().to_owned();
    let own = s.str("ai_embedding_base_url").trim();
    let Some(root) = base(if own.is_empty() { s.str("ai_base_url") } else { own }) else { return Err(unavailable()) };
    let client = client_for(state, &root).await?;
    let api_key = if own.is_empty()
        && std::env::var("BF_AI_EMBEDDING_API_KEY").is_err()
        && s.str("ai_embedding_api_key").is_empty()
    {
        key(state, "BF_AI_API_KEY", "ai_api_key")
    } else {
        key(state, "BF_AI_EMBEDDING_API_KEY", "ai_embedding_api_key")
    };
    let dims = s.int("ai_embedding_dims");
    let _permit = gate().acquire().await.map_err(AppError::internal)?;
    let mut out = Vec::with_capacity(inputs.len());
    for batch in inputs.chunks(16) {
        let mut body = json!({ "model": model, "input": batch });
        if dims > 0 {
            body["dimensions"] = json!(dims);
        }
        let mut req = client.post(format!("{root}/embeddings")).json(&body);
        if !api_key.is_empty() {
            req = req.bearer_auth(&api_key);
        }
        let res = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                let why = e.without_url().to_string();
                tracing::warn!(error = %why, "AI embeddings could not be reached");
                record(state, &c, &model, (0, 0), Some(&why)).await;
                return Err(unavailable());
            }
        };
        let status = res.status();
        let value: Value = res.json().await.unwrap_or(Value::Null);
        let data = value["data"].as_array().filter(|d| status.is_success() && d.len() == batch.len());
        let Some(data) = data else {
            let why = value.pointer("/error/message").and_then(Value::as_str).unwrap_or("no vectors came back");
            let why = format!("{} {why}", status.as_u16());
            tracing::warn!(error = %why, "AI embeddings failed");
            record(state, &c, &model, (0, 0), Some(&why)).await;
            return Err(unavailable());
        };
        for item in data {
            let mut v: Vec<f32> = item["embedding"]
                .as_array()
                .map(|a| a.iter().filter_map(serde_json::Value::as_f64).map(|x| x as f32).collect())
                .unwrap_or_default();
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if v.is_empty() || norm == 0.0 {
                record(state, &c, &model, (0, 0), Some("empty vector")).await;
                return Err(unavailable());
            }
            v.iter_mut().for_each(|x| *x /= norm);
            out.push(v);
        }
        let tokens = value["usage"]["total_tokens"].as_i64().or(value["usage"]["prompt_tokens"].as_i64()).unwrap_or(0);
        record(state, &c, &model, (tokens as i32, 0), None).await;
    }
    Ok(out)
}

/// The JSON object in a model's answer, which may come wrapped in prose or a code fence.
pub fn json_in(text: &str) -> Option<Value> {
    let from = text.find('{')?;
    let to = text.rfind('}')?;
    serde_json::from_str(text.get(from..=to)?).ok()
}

/// At most `n` characters, cut on a character boundary.
pub fn clip(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_wrapped_in_prose_still_parse() {
        let v = json_in("Sure!\n```json\n{\"spam\": 90, \"x\": [1]}\n```").unwrap();
        assert_eq!(v["spam"], 90);
        assert!(json_in("no object here").is_none());
    }

    #[test]
    fn provider_addresses_are_checked() {
        assert_eq!(base("https://api.openai.com/v1/").as_deref(), Some("https://api.openai.com/v1"));
        assert!(base("ftp://x/").is_none());
        assert!(base("https://user:pw@x/v1").is_none());
        assert!(base("not a url").is_none());
    }
}
