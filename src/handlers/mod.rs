pub mod admin;
pub mod airo;
pub mod auth;
pub mod connections;
pub mod device;
pub mod offices;
pub mod procedures;
pub mod threads;
pub mod updates;
pub mod web;
pub mod workflows;

use askama::Template;
use axum::extract::{FromRequest, Request};
use axum::http::header::CONTENT_TYPE;
use axum::http::HeaderMap;
use axum::response::Html;
use axum::{Form, Json};
use serde::de::DeserializeOwned;

use crate::error::AppError;

/// One handler serving both a browser's form POST and a JSON API caller's
/// POST, without hand-rolling two near-identical handler functions per
/// route: picks `Json<T>` or `Form<T>` (both axum's own, so both keep their
/// own content-type checks and error messages) by the request's
/// `Content-Type`, defaulting to form since that's what an HTML `<form>`
/// sends without the caller choosing anything.
pub struct JsonOrForm<T>(pub T);

impl<S, T> FromRequest<S> for JsonOrForm<T>
where
    S: Send + Sync,
    T: DeserializeOwned + 'static,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let is_json = req
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/json"));

        if is_json {
            let Json(value) = Json::<T>::from_request(req, state)
                .await
                .map_err(|_| AppError::BadRequest("invalid JSON body".to_string()))?;
            Ok(JsonOrForm(value))
        } else {
            let Form(value) = Form::<T>::from_request(req, state)
                .await
                .map_err(|_| AppError::BadRequest("invalid form body".to_string()))?;
            Ok(JsonOrForm(value))
        }
    }
}

/// Same route, JSON when the request asks for JSON, HTML when a browser
/// asks -- content negotiation on `Accept`, per the standing rule.
pub fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"))
}

/// An agent explicitly asking for unrendered content (a procedure body, a
/// comment, a thread note, a bio -- anything markdown, see `crate::markdown`)
/// gets the raw markdown back, not HTML and not JSON-wrapped-HTML. Kept
/// alongside `wants_html` since both read the same `Accept` header for the
/// same reason: the wire format follows what's asked for, not who's asking.
///
/// No route serves markdown yet -- Phase 1 is auth only -- so this has no
/// caller until one does.
#[allow(dead_code)]
pub fn wants_plain_text(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/plain"))
}

/// htmx sets this on every request it makes. A route that serves both a
/// full page and an htmx-swapped fragment of the same content should:
/// keep the fragment as its own template with no `{% extends "base.html" %}`,
/// have the full-page template `{% include %}` it, and pick which one to
/// render with this -- one route, one pair of templates, never a
/// hand-duplicated copy of the markup for each case. `handlers::threads`'
/// thread detail page (log + composer, self-polling) is the first user.
pub fn wants_fragment(headers: &HeaderMap) -> bool {
    headers
        .get("HX-Request")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// Instagram-style handle: lowercase, dash-separated, stable, typed after
/// an @. Every agent and device gets one so they can be searched, added,
/// and messaged by name.
pub fn slug_handle(name: &str) -> String {
    let mut s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "agent".to_string()
    } else {
        s
    }
}

/// A slug made unique across both registries (agents and devices).
pub async fn unique_handle(pool: &sqlx::PgPool, base: &str) -> String {
    let base = slug_handle(base);
    for attempt in 0..50 {
        let candidate = if attempt == 0 {
            base.clone()
        } else {
            format!("{base}-{attempt}")
        };
        let taken: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM agents WHERE handle = $1 AND deleted_at IS NULL)
                   + (SELECT COUNT(*) FROM nodes WHERE handle = $1 AND deleted_at IS NULL)",
        )
        .bind(&candidate)
        .fetch_one(pool)
        .await
        .unwrap_or(1);
        if taken == 0 {
            return candidate;
        }
    }
    format!("{}-{}", base, &uuid::Uuid::new_v4().to_string()[..4])
}

pub fn render<T: Template>(tpl: T) -> Result<Html<String>, AppError> {
    tpl.render().map(Html).map_err(|e| {
        tracing::error!(error = %e, "template render failed");
        AppError::Internal
    })
}

#[derive(Template)]
#[template(path = "message.html")]
struct MessageTemplate<'a> {
    title: &'a str,
    body: &'a str,
    active_tab: &'static str,
}

pub fn message_response(title: &str, body: &str) -> Result<Html<String>, AppError> {
    render(MessageTemplate {
        title,
        body,
        active_tab: "agents",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with(name: &str, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
        headers
    }

    #[test]
    fn wants_html_reads_accept() {
        assert!(wants_html(&headers_with("accept", "text/html,*/*")));
        assert!(!wants_html(&headers_with("accept", "application/json")));
        assert!(!wants_html(&HeaderMap::new()));
    }

    #[test]
    fn wants_plain_text_reads_accept() {
        assert!(wants_plain_text(&headers_with("accept", "text/plain")));
        assert!(!wants_plain_text(&headers_with("accept", "text/html")));
        assert!(!wants_plain_text(&HeaderMap::new()));
    }

    #[test]
    fn wants_fragment_reads_hx_request_header() {
        assert!(wants_fragment(&headers_with("HX-Request", "true")));
        assert!(wants_fragment(&headers_with("HX-Request", "TRUE")));
        assert!(!wants_fragment(&headers_with("HX-Request", "false")));
        assert!(!wants_fragment(&HeaderMap::new()));
    }

    /// Content negotiation is the whole safety boundary here: an agent
    /// asking for text/html would get markup and htmx attributes meant for
    /// browsers, and an agent asking for text/plain must never see either.
    /// `wants_html` and `wants_plain_text` are mutually exclusive for any
    /// real client, since a request only sends one of these Accept values.
    #[test]
    fn html_and_plain_text_negotiation_are_mutually_exclusive() {
        let html = headers_with("accept", "text/html");
        assert!(wants_html(&html));
        assert!(!wants_plain_text(&html));

        let plain = headers_with("accept", "text/plain");
        assert!(wants_plain_text(&plain));
        assert!(!wants_html(&plain));
    }
}
