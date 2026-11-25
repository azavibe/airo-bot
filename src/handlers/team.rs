//! Team invites and roles for an office (issue #29).
//!
//! The owner invites a teammate by email as a member (works with the bots,
//! approves) or a viewer (reads only). The email carries a link with a
//! one-time secret, stored only as an Argon2 hash; it is accepted by a
//! signed-in account with that same email, within 14 days. The owner
//! changes roles and removes people; anyone can leave.
//!
//! Routes:
//!   GET  /api/offices/:id/team                       everyone in the office
//!   POST /api/offices/:id/invites                    owner
//!   POST /api/invites/:id/revoke                     owner
//!   POST /api/invites/accept                         the invited account
//!   PATCH /api/offices/:id/members/:user_id          owner
//!   POST /api/offices/:id/members/:user_id/remove    owner, or the person
//!
//! TODO(air): a bot that uses one person's data (their Gmail, their
//! notes) in a shared chat should answer that person privately or ask
//! first; until then the office's connections stay the owner's.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::{wants_html, JsonOrForm};
use crate::auth::{secret, Caller};
use crate::error::AppError;
use crate::office::team::{require, role_in, Role};
use crate::state::AppState;

/// How long an invite link works.
const INVITE_DAYS: i64 = 14;
/// Open invites an office may have at once.
const MAX_OPEN_INVITES: i64 = 50;
/// People in an office besides its owner.
const MAX_MEMBERS: i64 = 50;

fn normal_email(raw: &str) -> Result<String, AppError> {
    let email = raw.trim().to_lowercase();
    let valid = email.len() <= 320
        && email.split_once('@').is_some_and(|(user, host)| {
            !user.is_empty() && host.contains('.') && !host.starts_with('.')
        })
        && !email.chars().any(|c| c.is_whitespace() || c.is_control());
    if !valid {
        return Err(AppError::BadRequest("enter an email address".into()));
    }
    Ok(email)
}

fn not_on_this_computer(state: &AppState) -> Result<(), AppError> {
    if state.config.local.is_some() {
        return Err(AppError::BadRequest(
            "Airobot on this computer has one person. To work with a team, use Airobot on a server."
                .into(),
        ));
    }
    Ok(())
}

#[derive(sqlx::FromRow, serde::Serialize)]
struct MemberRow {
    user_id: Uuid,
    handle: Option<String>,
    display_name: Option<String>,
    email: String,
    role: String,
    joined_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow, serde::Serialize)]
struct InviteRow {
    id: Uuid,
    email: String,
    role: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

/// GET /api/offices/:id/team -- who is in the office, and (for the owner)
/// the invites still open.
pub async fn team(
    State(state): State<AppState>,
    caller: Caller,
    Path(office): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let role = require(&state.pool, &caller, office, Role::Viewer).await?;
    let mut members = sqlx::query_as::<_, MemberRow>(
        "SELECT u.id AS user_id, u.handle, u.display_name, u.email, 'owner' AS role,
                w.created_at AS joined_at
         FROM workspaces w JOIN users u ON u.id = w.owner_user_id
         WHERE w.id = $1",
    )
    .bind(office)
    .fetch_all(&state.pool)
    .await?;
    members.extend(
        sqlx::query_as::<_, MemberRow>(
            "SELECT u.id AS user_id, u.handle, u.display_name, u.email, m.role,
                    m.created_at AS joined_at
             FROM workspace_members m
             JOIN workspaces w ON w.id = m.workspace_id
             JOIN users u ON u.id = m.user_id AND u.deleted_at IS NULL
             WHERE m.workspace_id = $1 AND m.deleted_at IS NULL AND m.user_id <> w.owner_user_id
             ORDER BY m.created_at ASC
             LIMIT 200",
        )
        .bind(office)
        .fetch_all(&state.pool)
        .await?,
    );
    let members: Vec<serde_json::Value> = members
        .into_iter()
        .map(|m| {
            json!({
                "user_id": m.user_id,
                "handle": m.handle,
                "display_name": m.display_name,
                // Addresses are the owner's to see; others see handles.
                "email": (role == Role::Owner || m.user_id == caller.user_id()).then_some(m.email),
                "role": m.role,
                "joined_at": m.joined_at,
                "you": m.user_id == caller.user_id(),
            })
        })
        .collect();
    let invites = if role == Role::Owner {
        sqlx::query_as::<_, InviteRow>(
            "SELECT id, email, role, expires_at, created_at FROM office_invites
             WHERE workspace_id = $1 AND deleted_at IS NULL AND accepted_at IS NULL
               AND expires_at > now()
             ORDER BY created_at DESC LIMIT 100",
        )
        .bind(office)
        .fetch_all(&state.pool)
        .await?
    } else {
        Vec::new()
    };
    Ok(Json(json!({
        "role": role.as_str(),
        "members": members,
        "invites": invites,
        "can_invite": role == Role::Owner && state.config.local.is_none(),
    })))
}

#[derive(Deserialize)]
pub struct InviteRequest {
    pub email: String,
    #[serde(default)]
    pub role: Option<String>,
    pub idempotency_key: Option<String>,
}

/// POST /api/offices/:id/invites -- invite someone by email. The answer
/// carries the link once (to copy); the email sends it too when the server
/// can send mail.
pub async fn invite(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(office): Path<Uuid>,
    JsonOrForm(body): JsonOrForm<InviteRequest>,
) -> Result<Response, AppError> {
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    require(&state.pool, &caller, office, Role::Owner).await?;
    not_on_this_computer(&state)?;
    let key = crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let email = normal_email(&body.email)?;
    let role = Role::invitable(body.role.as_deref().unwrap_or("member"))
        .ok_or_else(|| AppError::BadRequest("role must be member or viewer".into()))?;

    if let Some(key) = key {
        let earlier = sqlx::query_as::<_, InviteRow>(
            "SELECT id, email, role, expires_at, created_at FROM office_invites
             WHERE workspace_id = $1 AND idempotency_key = $2",
        )
        .bind(office)
        .bind(key)
        .fetch_optional(&state.pool)
        .await?;
        if let Some(invite) = earlier {
            // The link isn't kept, so a repeat can't show it again.
            return Ok(
                Json(json!({ "invite": invite, "link": null, "repeated": true })).into_response(),
            );
        }
    }

    if email == user.email.to_lowercase() {
        return Err(AppError::BadRequest("that's your own address".into()));
    }
    let already: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM workspace_members m JOIN users u ON u.id = m.user_id
            WHERE m.workspace_id = $1 AND m.deleted_at IS NULL AND lower(u.email) = $2)",
    )
    .bind(office)
    .bind(&email)
    .fetch_one(&state.pool)
    .await?;
    if already {
        return Err(AppError::Conflict("they're already in this office".into()));
    }
    let (open, members): (i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT COUNT(*) FROM office_invites WHERE workspace_id = $1 AND deleted_at IS NULL
               AND accepted_at IS NULL AND expires_at > now()),
            (SELECT COUNT(*) FROM workspace_members WHERE workspace_id = $1 AND deleted_at IS NULL)",
    )
    .bind(office)
    .fetch_one(&state.pool)
    .await?;
    if open >= MAX_OPEN_INVITES || members > MAX_MEMBERS {
        return Err(AppError::BadRequest(
            "this office has as many people and invites as it can hold".into(),
        ));
    }

    let id = Uuid::new_v4();
    let raw = secret::random_hex_token(32);
    let hash = secret::hash_secret(&raw).map_err(|e| {
        tracing::error!(error = %e, "could not hash an invite secret");
        AppError::Internal
    })?;
    let mut tx = state.pool.begin().await?;
    // A newer invite to the same address replaces the open one.
    sqlx::query(
        "UPDATE office_invites SET deleted_at = now(), updated_at = now()
         WHERE workspace_id = $1 AND lower(email) = $2 AND deleted_at IS NULL AND accepted_at IS NULL",
    )
    .bind(office)
    .bind(&email)
    .execute(&mut *tx)
    .await?;
    let invite = sqlx::query_as::<_, InviteRow>(
        "INSERT INTO office_invites (id, workspace_id, email, role, token_hash, invited_by,
                                     expires_at, idempotency_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id, email, role, expires_at, created_at",
    )
    .bind(id)
    .bind(office)
    .bind(&email)
    .bind(role.as_str())
    .bind(&hash)
    .bind(user.id)
    .bind(Utc::now() + Duration::days(INVITE_DAYS))
    .bind(key)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    let token = secret::encode_keyed_secret("", id, &raw);
    let link = format!(
        "{}/?invite={token}",
        state.config.app_base_url.trim_end_matches('/')
    );
    let office_name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = $1")
        .bind(office)
        .fetch_one(&state.pool)
        .await?;
    let inviter = user
        .display_name
        .clone()
        .or_else(|| user.handle.as_ref().map(|h| format!("@{h}")))
        .unwrap_or_else(|| user.email.clone());
    let what = if role == Role::Viewer {
        "see its chats, apps and approvals"
    } else {
        "work with its bots and approve their work"
    };
    let text = format!(
        "{inviter} invited you to {office_name} on Airobot, to {what}.\n\n\
         Open this link and sign in with {email} to join:\n{link}\n\n\
         The link works for {INVITE_DAYS} days. If you didn't expect this, ignore it."
    );
    let mut emailed = false;
    if !email.ends_with(".local") {
        match state
            .mailer
            .send_notice(&email, &format!("Join {office_name} on Airobot"), &text)
            .await
        {
            Ok(()) => emailed = true,
            Err(error) => tracing::warn!(%error, "could not email an invite"),
        }
    }
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({ "invite": invite, "link": link, "emailed": emailed })),
    )
        .into_response())
}

/// POST /api/invites/:id/revoke -- the link stops working.
pub async fn revoke_invite(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    crate::idempotency::key(&headers, None)?;
    let office: Uuid = sqlx::query_scalar(
        "SELECT workspace_id FROM office_invites WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    require(&state.pool, &caller, office, Role::Owner).await?;
    sqlx::query(
        "UPDATE office_invites SET deleted_at = now(), updated_at = now()
         WHERE id = $1 AND deleted_at IS NULL AND accepted_at IS NULL",
    )
    .bind(id)
    .execute(&state.pool)
    .await?;
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(Json(json!({ "status": "revoked", "id": id })).into_response())
}

#[derive(Deserialize)]
pub struct AcceptRequest {
    pub token: String,
    pub idempotency_key: Option<String>,
}

#[derive(sqlx::FromRow)]
struct InviteSecret {
    workspace_id: Uuid,
    email: String,
    role: String,
    token_hash: String,
    expires_at: DateTime<Utc>,
    accepted_by: Option<Uuid>,
    deleted_at: Option<DateTime<Utc>>,
}

/// POST /api/invites/accept -- join the office, as the account the invite
/// was sent to. Accepting the same invite again is harmless.
pub async fn accept_invite(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    JsonOrForm(body): JsonOrForm<AcceptRequest>,
) -> Result<Response, AppError> {
    let Caller::User(user) = &caller else {
        return Err(AppError::Forbidden);
    };
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let unusable =
        || AppError::BadRequest("This invite link doesn't work. Ask for a new one.".into());
    let (id, raw) = secret::decode_keyed_secret("", body.token.trim()).ok_or_else(unusable)?;
    let invite = sqlx::query_as::<_, InviteSecret>(
        "SELECT workspace_id, email, role, token_hash, expires_at, accepted_by, deleted_at
         FROM office_invites WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(unusable)?;
    if !secret::verify_secret(&raw, &invite.token_hash) {
        return Err(unusable());
    }
    let office: Option<(String, bool)> = sqlx::query_as(
        "SELECT name, owner_user_id = $2 FROM workspaces WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(invite.workspace_id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((office_name, is_owner)) = office else {
        return Err(AppError::BadRequest("That office no longer exists.".into()));
    };
    let joined = |role: &str| {
        json!({
            "status": "joined",
            "office": { "id": invite.workspace_id, "name": office_name },
            "role": role,
        })
    };
    // Already used by this person: say so again.
    if invite.accepted_by == Some(user.id) {
        let role = role_in(&state.pool, user.id, invite.workspace_id).await?;
        return match role {
            Some(role) => Ok(Json(joined(role.as_str())).into_response()),
            None => Err(AppError::BadRequest(
                "You were removed from that office. Ask for a new invite.".into(),
            )),
        };
    }
    if invite.deleted_at.is_some() || invite.accepted_by.is_some() {
        return Err(unusable());
    }
    if invite.expires_at <= Utc::now() {
        return Err(AppError::BadRequest(
            "This invite has expired. Ask for a new one.".into(),
        ));
    }
    if invite.email.to_lowercase() != user.email.to_lowercase() {
        return Err(AppError::BadRequest(format!(
            "This invite was sent to {}. Sign in with that address to accept it.",
            invite.email
        )));
    }
    if is_owner {
        return Err(AppError::BadRequest("You already own that office.".into()));
    }

    let mut tx = state.pool.begin().await?;
    let taken = sqlx::query(
        "UPDATE office_invites SET accepted_at = now(), accepted_by = $2, updated_at = now()
         WHERE id = $1 AND accepted_at IS NULL AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(user.id)
    .execute(&mut *tx)
    .await?;
    if taken.rows_affected() == 0 {
        return Err(unusable());
    }
    // A current membership keeps the higher of the two roles.
    let current: Option<String> = sqlx::query_scalar(
        "SELECT role FROM workspace_members
         WHERE workspace_id = $1 AND user_id = $2 AND deleted_at IS NULL",
    )
    .bind(invite.workspace_id)
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await?;
    let role = match current.as_deref() {
        Some("member") => "member".to_string(),
        Some(_) => {
            sqlx::query(
                "UPDATE workspace_members SET role = $3, updated_at = now()
                 WHERE workspace_id = $1 AND user_id = $2 AND deleted_at IS NULL",
            )
            .bind(invite.workspace_id)
            .bind(user.id)
            .bind(&invite.role)
            .execute(&mut *tx)
            .await?;
            invite.role.clone()
        }
        None => {
            sqlx::query(
                "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3)",
            )
            .bind(invite.workspace_id)
            .bind(user.id)
            .bind(&invite.role)
            .execute(&mut *tx)
            .await?;
            invite.role.clone()
        }
    };
    tx.commit().await?;
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(Json(joined(&role)).into_response())
}

#[derive(Deserialize)]
pub struct RoleRequest {
    pub role: String,
    pub idempotency_key: Option<String>,
}

/// PATCH /api/offices/:id/members/:user_id -- member or viewer.
pub async fn change_role(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path((office, person)): Path<(Uuid, Uuid)>,
    JsonOrForm(body): JsonOrForm<RoleRequest>,
) -> Result<Response, AppError> {
    require(&state.pool, &caller, office, Role::Owner).await?;
    crate::idempotency::key(&headers, body.idempotency_key.as_deref())?;
    let role = Role::invitable(body.role.trim())
        .ok_or_else(|| AppError::BadRequest("role must be member or viewer".into()))?;
    if person == caller.user_id() {
        return Err(AppError::BadRequest(
            "the owner's role doesn't change".into(),
        ));
    }
    let changed = sqlx::query(
        "UPDATE workspace_members SET role = $3, updated_at = now()
         WHERE workspace_id = $1 AND user_id = $2 AND deleted_at IS NULL",
    )
    .bind(office)
    .bind(person)
    .bind(role.as_str())
    .execute(&state.pool)
    .await?;
    if changed.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(
        Json(json!({ "status": "updated", "user_id": person, "role": role.as_str() }))
            .into_response(),
    )
}

/// POST /api/offices/:id/members/:user_id/remove -- the owner removes
/// someone, or a person leaves. Removing twice is harmless.
pub async fn remove_member(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    Path((office, person)): Path<(Uuid, Uuid)>,
) -> Result<Response, AppError> {
    crate::idempotency::key(&headers, None)?;
    let role = require(&state.pool, &caller, office, Role::Viewer).await?;
    let leaving = person == caller.user_id();
    if leaving && role == Role::Owner {
        return Err(AppError::BadRequest(
            "the owner can't leave their own office; delete it instead".into(),
        ));
    }
    if !leaving && role != Role::Owner {
        return Err(AppError::Forbidden);
    }
    sqlx::query(
        "UPDATE workspace_members SET deleted_at = now(), updated_at = now()
         WHERE workspace_id = $1 AND user_id = $2 AND deleted_at IS NULL
           AND user_id <> (SELECT owner_user_id FROM workspaces WHERE id = $1)",
    )
    .bind(office)
    .bind(person)
    .execute(&state.pool)
    .await?;
    if wants_html(&headers) {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(Json(json!({ "status": "removed", "user_id": person, "left": leaving })).into_response())
}

#[cfg(test)]
mod tests {
    use super::normal_email;

    #[test]
    fn emails_are_checked_and_lowercased() {
        assert_eq!(
            normal_email(" Ana@Example.COM ").ok().as_deref(),
            Some("ana@example.com")
        );
        for bad in [
            "",
            "ana",
            "@example.com",
            "ana@",
            "ana@example",
            "a na@example.com",
        ] {
            assert!(normal_email(bad).is_err(), "{bad}");
        }
    }
}
