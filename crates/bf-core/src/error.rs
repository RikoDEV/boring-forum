//! Error model: every failure leaves the server as RFC 9457 `application/problem+json`.

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("forbidden")]
    Forbidden,
    /// Allowed, but only after confirming the password again.
    #[error("sudo required")]
    SudoRequired,
    #[error("too many requests")]
    RateLimited { retry_after_secs: u64 },
    #[error("validation failed: {0}")]
    Validation(String),
    /// A validation error whose English text has `{name}` placeholders (`AppError::invalid`):
    /// sent with the template and its values, so it is translated before they are filled in.
    #[error("validation failed: {template}")]
    Invalid { template: &'static str, vars: Vec<(&'static str, String)> },
    /// Details are logged, never sent to the client.
    #[error("internal error")]
    Internal(String),
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Problem {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub title: &'static str,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// For `Invalid`: the English template and its values, for the `html_errors` middleware,
    /// which translates the detail and removes this before the response leaves.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub i18n: Option<serde_json::Value>,
}

impl AppError {
    pub fn internal(e: impl std::fmt::Display) -> Self {
        Self::Internal(e.to_string())
    }

    /// A validation error with values in its text: `invalid("At most {max} images.", &[("max", &5)])`.
    /// Write the template in English; `scripts/i18n.py` finds it for the catalogs.
    pub fn invalid(template: &'static str, vars: &[(&'static str, &dyn std::fmt::Display)]) -> Self {
        Self::Invalid { template, vars: vars.iter().map(|(k, v)| (*k, v.to_string())).collect() }
    }

    fn parts(&self) -> (StatusCode, &'static str, &'static str) {
        match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "not-found", "Not found"),
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad-request", "Bad request"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden", "Forbidden"),
            Self::SudoRequired => {
                (StatusCode::FORBIDDEN, "sudo-required", "Confirm your password at /sudo, then try again")
            }
            Self::RateLimited { .. } => (StatusCode::TOO_MANY_REQUESTS, "rate-limited", "Too many requests"),
            Self::Validation(_) | Self::Invalid { .. } => {
                (StatusCode::UNPROCESSABLE_ENTITY, "validation", "Validation failed")
            }
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal", "Internal server error"),
        }
    }

    pub fn problem(&self) -> Problem {
        let (status, kind, title) = self.parts();
        let detail = match self {
            Self::BadRequest(d) | Self::Validation(d) => Some(d.clone()),
            Self::Invalid { template, vars } => {
                let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
                Some(crate::mail::fill(template, &pairs))
            }
            _ => None,
        };
        let i18n = match self {
            Self::Invalid { template, vars } => Some(serde_json::json!({
                "template": template,
                "vars": vars.iter().map(|(k, v)| ((*k).to_owned(), serde_json::Value::from(v.as_str()))).collect::<serde_json::Map<_, _>>(),
            })),
            _ => None,
        };
        Problem { kind, title, status: status.as_u16(), detail, i18n }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if let Self::Internal(detail) = &self {
            tracing::error!(%detail, "internal error");
        }
        let problem = self.problem();
        let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::to_vec(&problem).unwrap_or_default();
        let mut res = (status, [(header::CONTENT_TYPE, "application/problem+json")], body).into_response();
        if let Self::RateLimited { retry_after_secs } = self {
            res.headers_mut().insert(header::RETRY_AFTER, retry_after_secs.into());
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_details_are_not_exposed() {
        let p = AppError::internal("db password wrong").problem();
        assert_eq!(p.status, 500);
        assert!(p.detail.is_none());
    }

    #[test]
    fn rate_limited_sets_retry_after() {
        let res = AppError::RateLimited { retry_after_secs: 7 }.into_response();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(res.headers()[header::RETRY_AFTER], "7");
        assert_eq!(res.headers()[header::CONTENT_TYPE], "application/problem+json");
    }
}
