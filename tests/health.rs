mod common;

#[sqlx::test]
async fn health_fails_when_database_is_unavailable(pool: PgPool) {
    pool.close().await;
    let res = send(pool, get_request("/health", None)).await;
    assert_eq!(res.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(res.headers.contains_key("x-request-id"));
    assert!(!res.text.contains("PoolClosed"));
}

use axum::http::StatusCode;
use common::{get_request, send};
use sqlx::PgPool;

#[sqlx::test]
async fn health_reports_ok(pool: PgPool) {
    let res = send(pool, get_request("/health", None)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.body["status"], "ok");
}

#[sqlx::test]
async fn web_logo_is_a_public_cacheable_svg(pool: PgPool) {
    let response = send(pool, get_request("/assets/airobot-logo.svg", None)).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response
            .headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("image/svg+xml; charset=utf-8")
    );
    assert_eq!(
        response
            .headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("public, max-age=86400")
    );
    assert!(response.text.starts_with("<svg"));
}

#[sqlx::test]
async fn llms_txt_is_served_for_crawling_agents(pool: PgPool) {
    let res = send(pool.clone(), get_request("/llms.txt", None)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(
        res.headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/plain; charset=utf-8")
    );
    assert!(res.text.contains("Air Noticeboard for AI Agents"));
    assert!(res.text.contains("/device/code"));
    assert!(res.text.contains("/agent/steps/current"));

    let res_full = send(pool, get_request("/llms-full.txt", None)).await;
    assert_eq!(res_full.status, StatusCode::OK);
    assert!(res_full.text.contains("Air Noticeboard for AI Agents"));
}

#[sqlx::test]
async fn agent_well_known_manifest_is_served(pool: PgPool) {
    let res = send(pool.clone(), get_request("/.well-known/agent.json", None)).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(
        res.headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(res.body["name_for_model"], "air_noticeboard");

    let res_alias = send(pool, get_request("/agent.json", None)).await;
    assert_eq!(res_alias.status, StatusCode::OK);
    assert_eq!(res_alias.body["name_for_model"], "air_noticeboard");
}

#[sqlx::test]
async fn every_response_carries_the_browser_hardening_headers(pool: PgPool) {
    let res = send(pool, get_request("/health", None)).await;
    assert_eq!(
        res.headers.get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(res.headers.get("x-frame-options").unwrap(), "DENY");
    assert_eq!(res.headers.get("referrer-policy").unwrap(), "same-origin");
}
