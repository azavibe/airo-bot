pub mod auth;
pub mod config;
pub mod connectors;
pub mod crypto;
pub mod db;
pub mod email;
pub mod error;
pub mod handlers;
pub mod idempotency;
pub mod local;
pub mod markdown;
pub mod models;
pub mod office;
pub mod openrouter;
pub mod push;
pub mod rate_limit;
pub mod state;
pub mod web;

use axum::extract::DefaultBodyLimit;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::IntoResponse;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde::Serialize;
use tower_governor::GovernorLayer;
use tower_http::services::{ServeDir, ServeFile};

use state::AppState;

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

async fn health(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<Health>, error::AppError> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        sqlx::query("SELECT 1").execute(&state.pool),
    )
    .await
    {
        Ok(Ok(_)) => Ok(Json(Health { status: "ok" })),
        _ => Err(error::AppError::ServiceUnavailable(
            "temporarily unavailable".into(),
        )),
    }
}

async fn request_context(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use tracing::Instrument;

    // Defense-in-depth CSRF mitigation: Reject cross-site state-changing browser submissions
    if (request.method() == axum::http::Method::POST
        || request.method() == axum::http::Method::DELETE
        || request.method() == axum::http::Method::PUT
        || request.method() == axum::http::Method::PATCH)
        && request
            .headers()
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == "cross-site")
    {
        tracing::warn!("Blocked cross-site state-changing request via Sec-Fetch-Site");
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }

    let request_id = uuid::Uuid::new_v4().to_string();
    // Never log the query string: magic links contain a credential there.
    let span = tracing::info_span!("request", %request_id, method = %request.method(), path = %request.uri().path());
    async move {
        let mut response = next.run(request).await;
        if let Ok(value) = request_id.parse() {
            response.headers_mut().insert("x-request-id", value);
        }
        // Cheap browser hardening on every response: no MIME sniffing, no
        // framing by other sites (clickjacking), no full URLs in Referer.
        // TODO(air): a Content-Security-Policy needs the voice engines' CDN
        // and wasm sources worked out first; add it once measured.
        for (name, value) in [
            ("x-content-type-options", "nosniff"),
            ("x-frame-options", "DENY"),
            ("referrer-policy", "same-origin"),
        ] {
            response
                .headers_mut()
                .entry(name)
                .or_insert(axum::http::HeaderValue::from_static(value));
        }
        // App pages are per-user and redirects are navigation, so neither may
        // be cached by browsers. A cached 301 is exactly how the pre-pivot
        // /yours -> /inbox redirect outlived its code on users' machines.
        // Anything that wants caching sets its own Cache-Control (e.g. the
        // logo's max-age) and keeps it.
        let is_html = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/html"));
        if response.status().is_redirection() || is_html {
            response
                .headers_mut()
                .entry(CACHE_CONTROL)
                .or_insert(axum::http::HeaderValue::from_static("no-store"));
        }
        tracing::info!(status = response.status().as_u16(), "request completed");
        response
    }
    .instrument(span)
    .await
}

async fn web_logo() -> impl IntoResponse {
    (
        [
            (CONTENT_TYPE, "image/svg+xml; charset=utf-8"),
            (CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_str!("../assets/airobot-logo.svg"),
    )
}

/// Serves the visionOS Obsidian Glass React SPA to browser requests for web routes,
/// while leaving API clients (Accept: application/json) completely uninterrupted.
async fn spa_navigation_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = request.method();
    let path = request.uri().path();
    let is_html_request = handlers::wants_html(request.headers());

    // In unit / integration test mode (where Config.database_url is empty in tests/common),
    // allow handlers to render server-side Askama templates directly so tests verify contracts.
    let is_test_environment = state.config.database_url.is_empty();

    let is_spa_route = is_spa_route(path);

    if !is_test_environment && method == axum::http::Method::GET && is_html_request && is_spa_route
    {
        if let Some(index_html) = spa_index(&state.config.web_dist, state.config.production).await {
            return (
                [
                    (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
                    (
                        axum::http::header::CACHE_CONTROL,
                        "no-cache, no-store, must-revalidate",
                    ),
                ],
                index_html,
            )
                .into_response();
        }
    }

    next.run(request).await
}

/// Browser pages owned by the React app. Each matches a screen the SPA can
/// show, so reloading, Back/Forward, or a redirect (sign-in lands on
/// `/inbox`) never drops the user into a different-looking legacy page.
/// JSON clients on the same paths are untouched (the middleware only
/// answers HTML GETs).
fn is_spa_route(path: &str) -> bool {
    const SECTIONS: &[&str] = &[
        "/chat",
        "/onboard",
        "/apps",
        "/dash",
        "/rooms",
        "/workflows",
        "/procedures",
        "/approvals",
        "/bots",
        "/contacts",
        "/inbox",
        "/signin",
        "/admin",
        "/connections",
        // Settings is a page of the app (account, appearance, voice,
        // connections, admin); its forms still post to the server routes.
        "/settings",
    ];
    path == "/"
        || SECTIONS.iter().any(|section| {
            path == *section
                || path
                    .strip_prefix(section)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}

/// The SPA shell. Production reads it once (the image is immutable);
/// development re-reads it so a fresh `npm run build` shows up immediately.
async fn spa_index(web_dist: &std::path::Path, production: bool) -> Option<String> {
    static INDEX: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if production {
        if let Some(cached) = INDEX.get() {
            return Some(cached.clone());
        }
    }
    let html = tokio::fs::read_to_string(web_dist.join("index.html"))
        .await
        .ok()?;
    if production {
        let _ = INDEX.set(html.clone());
    }
    Some(html)
}

const ONE_MB: usize = 1024 * 1024;

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::web::feed))
        .route("/health", get(health))
        .route("/llms.txt", get(handlers::web::llms_txt))
        .route("/llms-full.txt", get(handlers::web::llms_txt))
        .route(
            "/.well-known/agent.json",
            get(handlers::web::agent_well_known),
        )
        .route("/agent.json", get(handlers::web::agent_well_known))
        .route("/assets/airobot-logo.svg", get(web_logo))
        .route(
            "/signin",
            get(handlers::auth::login_form)
                .post(handlers::auth::request_login)
                .layer(GovernorLayer {
                    config: rate_limit::signin(),
                }),
        )
        .route("/auth/callback", get(handlers::auth::callback))
        .route("/auth/qr", get(handlers::auth::qr_callback))
        .route("/api/auth/qr-link", post(handlers::auth::create_qr_link))
        .route("/auth/logout", post(handlers::auth::logout))
        .route(
            "/onboarding/handle",
            get(handlers::auth::handle_form).post(handlers::auth::set_handle),
        )
        .route("/device/code", post(handlers::device::create_code))
        .route("/device/token", post(handlers::device::poll_token))
        .route("/device", get(handlers::device::verify_form))
        .route("/device/approve", post(handlers::device::approve))
        .route("/device/deny", post(handlers::device::deny))
        .route(
            "/threads",
            get(handlers::threads::list_threads).post(handlers::threads::create_thread),
        )
        .route("/threads/{id}", get(handlers::threads::get_thread))
        .route("/t/{id}", get(handlers::threads::get_thread))
        .route(
            "/threads/{id}/events",
            get(handlers::threads::thread_live_events),
        )
        .route("/t/{id}/events", get(handlers::threads::thread_live_events))
        .route("/threads/{id}/notes", post(handlers::threads::create_note))
        .route(
            "/threads/{id}/rename",
            post(handlers::threads::rename_thread),
        )
        .route(
            "/threads/{id}/delete",
            post(handlers::threads::delete_thread),
        )
        .route("/notes/{id}/delete", post(handlers::threads::delete_note))
        .route(
            "/threads/{id}/archive",
            post(handlers::threads::archive_thread),
        )
        .route(
            "/threads/{id}/unarchive",
            post(handlers::threads::unarchive_thread),
        )
        .route("/connect", get(handlers::web::connect_guide))
        .route("/inbox", get(handlers::threads::inbox))
        .route(
            "/post",
            get(handlers::web::post_form).post(handlers::web::create_post),
        )
        .route(
            "/procedures",
            get(handlers::procedures::list_procedures).post(handlers::procedures::create_procedure),
        )
        .route(
            "/procedures/{handle}/{slug}",
            get(handlers::procedures::get_procedure),
        )
        .route(
            "/u/{handle}/{slug}",
            get(handlers::procedures::get_procedure),
        )
        .route(
            "/procedures/{id}",
            patch(handlers::procedures::update_procedure),
        )
        .route(
            "/procedures/{id}/publish",
            post(handlers::procedures::publish_procedure),
        )
        .route(
            "/procedures/{id}/stats",
            get(handlers::procedures::get_stats),
        )
        .route(
            "/procedures/{id}/comments",
            post(handlers::procedures::create_comment),
        )
        .route(
            "/procedures/{id}/send",
            post(handlers::procedures::send_to_agent),
        )
        .route(
            "/procedures/{id}/fork",
            post(handlers::procedures::fork_procedure),
        )
        .route(
            "/procedures/{id}/delete",
            post(handlers::procedures::delete_procedure),
        )
        .route("/runs", post(handlers::procedures::create_run))
        .route("/inbox/attention", get(handlers::web::inbox_attention))
        .route("/yours", get(handlers::web::yours_redirect))
        .route(
            "/yours/attention",
            get(handlers::web::yours_attention_redirect),
        )
        .route(
            "/tasks",
            get(handlers::airo::list_tasks).post(handlers::airo::create_task),
        )
        .route("/tasks/{id}", get(handlers::airo::task_detail))
        .route("/tasks/{id}/claim", post(handlers::airo::claim_task))
        .route("/tasks/{id}/assign", post(handlers::airo::assign_task))
        .route("/tasks/{id}/result", post(handlers::airo::post_task_result))
        .route("/bots", get(handlers::airo::agents_overview))
        .route("/nodes", post(handlers::airo::register_node))
        .route("/approvals", get(handlers::airo::list_approvals))
        .route("/admin", get(handlers::admin::admin_home))
        .route("/admin/settings", post(handlers::admin::admin_settings))
        .route(
            "/admin/users/{id}/admin",
            post(handlers::admin::toggle_admin),
        )
        .route(
            "/admin/agents/{id}/revoke",
            post(handlers::admin::revoke_agent),
        )
        .route("/onboarding/state", get(handlers::admin::onboarding_state))
        .route("/onboarding/done", post(handlers::admin::onboarding_done))
        .route(
            "/approvals/{id}/decide",
            post(handlers::airo::decide_approval),
        )
        .route("/activity", get(handlers::airo::list_activity))
        .route("/agent/tasks/poll", get(handlers::airo::agent_poll_tasks))
        .route("/agent/chat-route", post(handlers::airo::set_chat_route))
        .route(
            "/boards",
            get(handlers::web::boards).post(handlers::web::create_board),
        )
        .route("/b/{name}", get(handlers::web::board))
        .route(
            "/boards/{name}",
            get(handlers::web::board).patch(handlers::workflows::edit_board),
        )
        .route("/boards/{name}/edit", post(handlers::workflows::edit_board))
        .route(
            "/boards/{name}/threads",
            post(handlers::web::create_workspace_thread),
        )
        .route(
            "/boards/{name}/agents",
            post(handlers::web::assign_board_agent),
        )
        .route(
            "/boards/{name}/agents/{agent_id}/remove",
            post(handlers::web::remove_board_agent),
        )
        .route("/boards/{name}/join", post(handlers::web::join_board))
        .route("/boards/{name}/leave", post(handlers::web::leave_board))
        .route(
            "/workflows",
            get(handlers::workflows::list_workflows).post(handlers::workflows::create_workflow),
        )
        .route(
            "/workflows/new",
            get(handlers::workflows::new_workflow_form),
        )
        .route(
            "/workflows/{id}",
            get(handlers::workflows::get_workflow).patch(handlers::workflows::update_workflow),
        )
        .route(
            "/workflows/{id}/edit",
            get(handlers::workflows::edit_workflow_form).post(handlers::workflows::update_workflow),
        )
        .route(
            "/workflows/{id}/delete",
            post(handlers::workflows::delete_workflow),
        )
        .route(
            "/workflows/{id}/steps/{step_order}/assign",
            post(handlers::workflows::assign_workflow_step),
        )
        .route(
            "/workflows/{id}/runs",
            post(handlers::workflows::start_workflow_run),
        )
        .route(
            "/procedures/{id}/runs",
            post(handlers::workflows::start_workflow_run),
        )
        .route("/runs/{id}", get(handlers::workflows::get_run))
        .route(
            "/runs/{run_id}/steps/{step_order}/result",
            post(handlers::workflows::submit_step_result),
        )
        .route(
            "/runs/{run_id}/steps/{step_order}/run-openrouter",
            post(handlers::workflows::run_step_openrouter),
        )
        .route(
            "/agents",
            get(handlers::airo::agents_overview).post(handlers::airo::create_agent),
        )
        .route(
            "/agent/steps/current",
            get(handlers::workflows::agent_current_step),
        )
        .route(
            "/agent/heartbeat",
            post(handlers::workflows::agent_heartbeat).get(handlers::workflows::agent_heartbeat),
        )
        .route("/settings", get(handlers::web::settings))
        .route("/settings/agents", get(handlers::web::agents))
        .route(
            "/settings/agents/openrouter",
            post(handlers::web::connect_openrouter_agent),
        )
        .route(
            "/connect/openrouter",
            post(handlers::web::connect_openrouter_agent),
        )
        .route("/settings/theme", post(handlers::web::set_theme))
        .route(
            "/settings/agents/{id}/revoke",
            post(handlers::web::revoke_agent),
        )
        .route("/agents/{id}/delete", post(handlers::web::delete_agent))
        .route(
            "/api/offices/{id}/memory",
            get(office::memory::list_memory).post(office::memory::create_memory),
        )
        .route("/api/memories/{id}", patch(office::memory::update_memory))
        .route(
            "/api/memories/{id}/delete",
            post(office::memory::delete_memory),
        )
        .route(
            "/api/lessons/{id}/delete",
            post(office::memory::delete_lesson),
        )
        .route(
            "/agents/{id}/access",
            get(office::access::get_access).put(office::access::set_access),
        )
        .route("/agents/{id}/stats", get(office::spend::agent_stats))
        .route(
            "/agents/{id}/schedules",
            get(office::recurring::list_schedules).post(office::recurring::create_schedule),
        )
        .route(
            "/api/schedules/{id}",
            patch(office::recurring::update_schedule),
        )
        .route(
            "/api/schedules/{id}/delete",
            post(office::recurring::delete_schedule),
        )
        .route(
            "/agents/{id}/access/reset",
            post(office::access::reset_access),
        )
        .route(
            "/api/offices/{id}/contacts",
            get(office::contacts::list_contacts).post(office::contacts::create_contact),
        )
        .route(
            "/api/offices/{id}/contacts/import",
            post(office::contacts::import_contacts),
        )
        .route(
            "/api/contacts/{id}",
            get(office::contacts::get_contact).patch(office::contacts::update_contact),
        )
        .route(
            "/api/contacts/{id}/delete",
            post(office::contacts::delete_contact),
        )
        .route(
            "/api/offices/{id}/documents",
            get(office::documents::list_documents)
                .post(office::documents::create_document)
                // Uploads carry a file in base64; every other route keeps 1 MB.
                .layer(DefaultBodyLimit::max(office::documents::UPLOAD_BODY_LIMIT)),
        )
        .route("/api/offices/{id}/goal", get(office::goals::get_goal))
        .route(
            "/api/offices/{id}/milestones",
            post(office::goals::create_milestone),
        )
        .route(
            "/api/milestones/{id}",
            patch(office::goals::update_milestone),
        )
        .route(
            "/api/milestones/{id}/delete",
            post(office::goals::delete_milestone),
        )
        .route(
            "/api/offices/{id}/documents/search",
            get(office::documents::search_documents),
        )
        .route(
            "/api/documents/{id}",
            get(office::documents::get_document).patch(office::documents::update_document),
        )
        .route(
            "/api/documents/{id}/delete",
            post(office::documents::delete_document),
        )
        .route(
            "/api/offices/{id}/document-folders",
            get(office::documents::list_folders).post(office::documents::link_folder),
        )
        .route(
            "/api/document-folders/{id}/sync",
            post(office::documents::sync_folder_now),
        )
        .route(
            "/api/document-folders/{id}/delete",
            post(office::documents::delete_folder),
        )
        .route(
            "/api/documents/{id}/sync",
            post(office::documents::sync_document),
        )
        .route(
            "/api/documents/{id}/outdated",
            post(office::documents::mark_outdated),
        )
        .route(
            "/api/contacts/{id}/memory",
            post(office::contacts::add_contact_memory),
        )
        .route(
            "/settings/agents/{id}/rename",
            post(handlers::web::rename_agent),
        )
        .route(
            "/agents/{id}/settings",
            post(handlers::web::update_agent_settings),
        )
        .route(
            "/connect/agents/{id}/rename",
            post(handlers::web::rename_agent),
        )
        .route(
            "/settings/agents/{id}/chat",
            post(handlers::web::chat_with_agent),
        )
        .route(
            "/connect/agents/{id}/chat",
            post(handlers::web::chat_with_agent),
        )
        .route("/notifs", get(handlers::web::notifications))
        .route(
            "/notifs/read-all",
            post(handlers::web::mark_all_notifications_read),
        )
        .route(
            "/notifs/{id}/read",
            post(handlers::web::mark_notification_read),
        )
        .route("/sw.js", get(handlers::web::service_worker))
        .route("/push/public-key", get(handlers::web::push_public_key))
        .route(
            "/push/subscriptions",
            post(handlers::web::subscribe_push).delete(handlers::web::unsubscribe_push),
        )
        // --- Airobot Virtual Office Routes ---
        .route("/api/me", get(handlers::auth::current_user))
        .route("/api/me/theme", put(handlers::auth::save_theme))
        .route("/api/me/language", put(handlers::auth::save_language))
        .route("/api/app/update", get(handlers::updates::check_update))
        .route(
            "/api/languages",
            get(crate::office::language::list_languages),
        )
        .route("/api/user/me", get(handlers::auth::current_user))
        .route("/api/auth/dev-login", post(handlers::auth::dev_login))
        .route(
            "/api/offices",
            get(handlers::offices::list_offices).post(handlers::offices::create_office),
        )
        .route("/api/offices/{id}", patch(handlers::offices::rename_office))
        .route(
            "/api/offices/{id}/rename",
            post(handlers::offices::rename_office),
        )
        .route(
            "/api/offices/{id}/delete",
            post(handlers::offices::delete_office),
        )
        .route(
            "/api/connections",
            get(handlers::connections::list_connections),
        )
        .route(
            "/api/connections/webhook",
            post(handlers::connections::create_webhook),
        )
        .route(
            "/api/connections/oauth/{provider}/start",
            post(handlers::connections::start_oauth),
        )
        .route(
            "/api/connections/oauth/{provider}/callback",
            get(handlers::connections::oauth_callback),
        )
        .route(
            "/api/connections/apps",
            get(handlers::connections::list_connector_apps),
        )
        .route(
            "/api/connections/apps/{provider}",
            post(handlers::connections::save_connector_app),
        )
        .route(
            "/api/connections/apps/{provider}/delete",
            post(handlers::connections::delete_connector_app),
        )
        .route(
            "/api/connections/key/{provider}",
            post(handlers::connections::connect_with_key),
        )
        .route(
            "/api/connections/{id}/delete",
            post(handlers::connections::delete_connection),
        )
        .route(
            "/api/connections/{id}/actions/{action}",
            post(handlers::connections::run_action),
        )
        .route(
            "/api/hooks/{id}/{token}",
            post(handlers::connections::receive_hook),
        )
        .route(
            "/api/office/onboard",
            post(office::architect::onboard_workspace),
        )
        .route(
            "/api/office/apps",
            get(office::architect::list_office_apps).post(office::architect::create_office_app),
        )
        .route(
            "/api/office/apps/install",
            post(office::architect::install_office_app),
        )
        .route(
            "/api/office/apps/{slug}/records",
            get(office::architect::get_app_records).post(office::architect::create_app_record),
        )
        .route(
            "/api/office/records/{id}",
            patch(office::architect::patch_app_record),
        )
        .route("/api/office/records", get(office::records::get_records))
        .route(
            "/api/office/records/{id}/convert",
            post(office::records::convert_record),
        )
        .route("/api/office/events", get(office::records::office_events))
        .route(
            "/api/office/records/{id}/delete",
            post(office::architect::delete_app_record),
        )
        .route(
            "/api/office/apps/{slug}/archive",
            post(office::architect::archive_office_app),
        )
        .route(
            "/api/office/apps/{slug}/unarchive",
            post(office::architect::unarchive_office_app),
        )
        .route(
            "/api/office/apps/{slug}/delete",
            post(office::architect::delete_office_app),
        )
        .route(
            "/api/office/wallet",
            get(office::architect::get_workspace_wallet),
        )
        .route(
            "/api/office/apps/{slug}/schema",
            put(office::architect::update_mod_schema),
        )
        .route(
            "/api/office/apps/{slug}/reset",
            post(office::architect::reset_mod_schema),
        )
        .route(
            "/api/workspace/lessons",
            get(office::architect::list_workspace_lessons)
                .post(office::architect::create_workspace_lesson),
        )
        .route(
            "/api/bot/local-status",
            get(handlers::airo::get_local_bot_status),
        )
        .route(
            "/api/bot/local-toggle",
            post(handlers::airo::set_local_bot_toggle),
        )
        .route(
            "/api/procedures/{id}/evolve",
            post(office::evolver::evolve_procedure),
        )
        .route(
            "/api/webhooks/io/{task_id}",
            post(office::fsm_dispatcher::handle_io_webhook),
        )
        .route("/reactions", post(handlers::web::react))
        .route("/users/{id}/follow", post(handlers::web::follow))
        .route("/users/{id}/unfollow", post(handlers::web::unfollow))
        .route("/u/{handle}", get(handlers::web::profile))
        .route("/{profile}", get(handlers::web::at_profile))
        .route("/local/enter", get(local::enter))
        .fallback_service(
            ServeDir::new(&state.config.web_dist)
                .not_found_service(ServeFile::new(state.config.web_dist.join("index.html"))),
        )
        .layer(GovernorLayer {
            config: rate_limit::build(),
        })
        .layer(DefaultBodyLimit::max(ONE_MB))
        .layer(axum::middleware::from_fn(request_context))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            spa_navigation_middleware,
        ))
        // Outermost: in local mode nothing reaches the app unless it was
        // addressed to this computer's loopback port.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            local::host_guard,
        ))
        .with_state(state)
}

#[cfg(test)]
mod spa_route_tests {
    use super::is_spa_route;

    #[test]
    fn app_screens_are_served_by_the_spa() {
        for path in [
            "/",
            "/apps",
            "/apps/crm",
            "/approvals",
            "/bots",
            "/workflows",
            "/procedures/abc",
            "/inbox",
            "/signin",
            "/admin",
            "/connections",
            "/settings",
            "/settings/connections",
        ] {
            assert!(is_spa_route(path), "{path} should load the app");
        }
    }

    #[test]
    fn other_pages_keep_their_server_rendering() {
        for path in [
            "/device",
            "/settingsx",
            "/onboarding/handle",
            "/inboxes",
            "/appsx",
            "/auth/callback",
            "/b/general",
            "/administrator",
        ] {
            assert!(!is_spa_route(path), "{path} should not load the app");
        }
    }
}
