//! Who may do what in an office (issue #29). The office's owner
//! (`workspaces.owner_user_id`) can do everything; people they invite are
//! members (chat with the bots, approve) or viewers (read only). Everything
//! an office holds still belongs to the owner's account: bots answer on the
//! owner's AI and credits whoever writes to them.

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Caller;
use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Member,
    Owner,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Member => "member",
            Role::Owner => "owner",
        }
    }

    /// A role an invite or a change may give (never owner).
    pub fn invitable(raw: &str) -> Option<Role> {
        match raw {
            "member" => Some(Role::Member),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }

    fn stored(raw: &str) -> Role {
        match raw {
            "owner" => Role::Owner,
            "viewer" => Role::Viewer,
            // Rows from before roles were checked are members.
            _ => Role::Member,
        }
    }

    /// May write to the office's chats and decide its approvals.
    pub fn can_act(self) -> bool {
        self >= Role::Member
    }
}

/// `user_id`'s role in `office`, or None when they have no access (or the
/// office is gone). A removed member has none.
pub async fn role_in(pool: &PgPool, user_id: Uuid, office: Uuid) -> Result<Option<Role>, AppError> {
    let row: Option<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT w.owner_user_id,
                (SELECT m.role FROM workspace_members m
                  WHERE m.workspace_id = w.id AND m.user_id = $2 AND m.deleted_at IS NULL
                  LIMIT 1)
         FROM workspaces w
         WHERE w.id = $1 AND w.deleted_at IS NULL",
    )
    .bind(office)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        None => None,
        Some((owner, _)) if owner == user_id => Some(Role::Owner),
        Some((_, Some(role))) => Some(Role::stored(&role)),
        Some((_, None)) => None,
    })
}

/// The caller's role in `office`, at least `needed`. No access at all is
/// "not found" (an office's existence isn't told to strangers); too low a
/// role is "forbidden". A bot acts with its owner's role.
pub async fn require(
    pool: &PgPool,
    caller: &Caller,
    office: Uuid,
    needed: Role,
) -> Result<Role, AppError> {
    let role = role_in(pool, caller.user_id(), office)
        .await?
        .ok_or(AppError::NotFound)?;
    if role < needed {
        return Err(AppError::Forbidden);
    }
    Ok(role)
}

/// `require`, for the signed-in person only: sharing the office (invites,
/// roles) and connecting outside channels (Telegram, Slack, a phone
/// number) never happen with a bot's key, even its owner's, so a leaked
/// key can't hand the office's messages to someone else.
pub async fn require_person(
    pool: &PgPool,
    caller: &Caller,
    office: Uuid,
    needed: Role,
) -> Result<Role, AppError> {
    if !matches!(caller, Caller::User(_)) {
        return Err(AppError::Forbidden);
    }
    require(pool, caller, office, needed).await
}

/// The offices `user_id` was invited to (not their own), with their role.
pub async fn shared_offices(pool: &PgPool, user_id: Uuid) -> Result<Vec<(Uuid, Role)>, AppError> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT m.workspace_id, m.role FROM workspace_members m
         JOIN workspaces w ON w.id = m.workspace_id
         WHERE m.user_id = $1 AND m.deleted_at IS NULL AND w.deleted_at IS NULL
           AND w.owner_user_id <> $1",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, role)| (id, Role::stored(&role)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered_and_only_two_are_invitable() {
        assert!(Role::Owner > Role::Member && Role::Member > Role::Viewer);
        assert!(Role::Member.can_act() && !Role::Viewer.can_act());
        assert_eq!(Role::invitable("member"), Some(Role::Member));
        assert_eq!(Role::invitable("viewer"), Some(Role::Viewer));
        assert_eq!(Role::invitable("owner"), None);
        assert_eq!(Role::stored("anything else"), Role::Member);
    }
}
