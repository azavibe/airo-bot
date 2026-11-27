//! Encryption at rest for third-party credentials Air must be able to use
//! again (an agent's own OpenRouter key). Air-issued secrets stay Argon2
//! hashes; this is only for keys the server has to present to a provider.

use std::sync::OnceLock;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use sha2::{Digest, Sha256};

const PREFIX: &str = "enc:v1:";
/// Development-only fallback so local runs and tests work without setup.
/// Production refuses to start without PROVIDER_KEY_SECRET (see config.rs).
const DEV_FALLBACK_SECRET: &str = "airobot-development-only-provider-key-secret";

fn cipher() -> &'static Aes256Gcm {
    static CIPHER: OnceLock<Aes256Gcm> = OnceLock::new();
    CIPHER.get_or_init(|| {
        let secret = std::env::var("PROVIDER_KEY_SECRET")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| {
                tracing::warn!("PROVIDER_KEY_SECRET unset; using the development-only key");
                DEV_FALLBACK_SECRET.to_string()
            });
        let key_bytes: [u8; 32] = Sha256::digest(secret.trim().as_bytes()).into();
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes))
    })
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

pub fn is_encrypted(stored: &str) -> bool {
    stored.starts_with(PREFIX)
}

/// Encrypts a provider key for storage. `None` only if the cipher fails,
/// which callers treat as an internal error rather than storing plaintext.
pub fn encrypt(plain: &str) -> Option<String> {
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher().encrypt(&nonce, plain.as_bytes()).ok()?;
    let mut bytes = nonce.to_vec();
    bytes.extend_from_slice(&ciphertext);
    Some(format!("{PREFIX}{}", to_hex(&bytes)))
}

/// Decrypts a stored provider key. Rows written before encryption existed
/// are migrated at startup, but a plaintext value is still returned as-is
/// so a key is never silently lost.
pub fn decrypt(stored: &str) -> Option<String> {
    let Some(hex) = stored.strip_prefix(PREFIX) else {
        return Some(stored.to_string());
    };
    let bytes = from_hex(hex)?;
    if bytes.len() < 12 {
        return None;
    }
    let (nonce, ciphertext) = bytes.split_at(12);
    let plain = cipher()
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .ok()?;
    String::from_utf8(plain).ok()
}

fn server_secret() -> String {
    std::env::var("PROVIDER_KEY_SECRET")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEV_FALLBACK_SECRET.to_string())
}

fn webhook_mac(task_id: uuid::Uuid, job_id: &str) -> hmac::Hmac<Sha256> {
    use hmac::Mac;
    // Domain-separated from the provider-key cipher key.
    let key: [u8; 32] = Sha256::digest(format!("webhook-v1:{}", server_secret().trim())).into();
    let mut mac = <hmac::Hmac<Sha256> as Mac>::new_from_slice(&key)
        .unwrap_or_else(|_| unreachable!("HMAC accepts any key length"));
    mac.update(task_id.as_bytes());
    mac.update(b":");
    mac.update(job_id.as_bytes());
    mac
}

/// The token an external provider presents when calling back
/// `/api/webhooks/io/{task_id}`. Derived (HMAC of task id and the current
/// external job id) rather than stored, so no raw webhook secret is ever
/// persisted and a token dies with its job.
pub fn webhook_token(task_id: uuid::Uuid, job_id: &str) -> String {
    use hmac::Mac;
    to_hex(&webhook_mac(task_id, job_id).finalize().into_bytes())
}

pub fn verify_webhook_token(task_id: uuid::Uuid, job_id: &str, token: &str) -> bool {
    use hmac::Mac;
    let Some(bytes) = from_hex(token.trim()) else {
        return false;
    };
    webhook_mac(task_id, job_id).verify_slice(&bytes).is_ok()
}

fn derived_mac(purpose: &str) -> hmac::Hmac<Sha256> {
    use hmac::Mac;
    // Domain-separated per purpose from the cipher key and the task tokens.
    let key: [u8; 32] = Sha256::digest(format!("{purpose}:{}", server_secret().trim())).into();
    <hmac::Hmac<Sha256> as Mac>::new_from_slice(&key)
        .unwrap_or_else(|_| unreachable!("HMAC accepts any key length"))
}

fn connection_hook_mac(connection_id: uuid::Uuid, salt: &str) -> hmac::Hmac<Sha256> {
    use hmac::Mac;
    let mut mac = derived_mac("connection-hook-v1");
    mac.update(connection_id.as_bytes());
    mac.update(b":");
    mac.update(salt.as_bytes());
    mac
}

/// The secret part of a webhook connection's inbound URL. Derived from the
/// connection id and a stored, non-secret salt, so the URL can be shown to
/// its owner again without storing it, and changes if the salt does.
pub fn connection_hook_token(connection_id: uuid::Uuid, salt: &str) -> String {
    use hmac::Mac;
    to_hex(
        &connection_hook_mac(connection_id, salt)
            .finalize()
            .into_bytes(),
    )
}

pub fn verify_connection_hook_token(connection_id: uuid::Uuid, salt: &str, token: &str) -> bool {
    use hmac::Mac;
    let Some(bytes) = from_hex(token.trim()) else {
        return false;
    };
    connection_hook_mac(connection_id, salt)
        .verify_slice(&bytes)
        .is_ok()
}

/// The verify token Meta presents when checking an office's WhatsApp
/// webhook. Derived from the number's row id, so it can be shown to the
/// owner again without being stored, and changes when they reconnect.
pub fn whatsapp_verify_token(number_id: uuid::Uuid) -> String {
    use hmac::Mac;
    let mut mac = derived_mac("whatsapp-verify-v1");
    mac.update(number_id.as_bytes());
    to_hex(&mac.finalize().into_bytes())
}

pub fn verify_whatsapp_token(number_id: uuid::Uuid, token: &str) -> bool {
    use hmac::Mac;
    let Some(bytes) = from_hex(token.trim()) else {
        return false;
    };
    let mut mac = derived_mac("whatsapp-verify-v1");
    mac.update(number_id.as_bytes());
    mac.verify_slice(&bytes).is_ok()
}

/// The PKCE verifier for an OAuth sign-in, derived from the state's secret
/// so it never has to be stored. 64 hex characters, as RFC 7636 allows.
pub fn pkce_verifier(state_secret: &str) -> String {
    use hmac::Mac;
    let mut mac = derived_mac("oauth-pkce-v1");
    mac.update(state_secret.as_bytes());
    to_hex(&mac.finalize().into_bytes())
}

/// Encrypts every provider key still stored in plaintext. Runs at startup.
pub async fn encrypt_legacy_provider_keys(pool: &sqlx::PgPool) -> Result<u64, sqlx::Error> {
    let rows: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, provider_api_key FROM agents
         WHERE provider_api_key IS NOT NULL AND provider_api_key <> ''
           AND provider_api_key NOT LIKE 'enc:v1:%'",
    )
    .fetch_all(pool)
    .await?;
    let mut migrated = 0;
    for (id, plain) in rows {
        let Some(sealed) = encrypt(&plain) else {
            continue;
        };
        migrated += sqlx::query(
            "UPDATE agents SET provider_api_key = $1, updated_at = now()
             WHERE id = $2 AND provider_api_key = $3",
        )
        .bind(sealed)
        .bind(id)
        .bind(&plain)
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(migrated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_legacy_plaintext() {
        let sealed = encrypt("sk-or-test-123").expect("encrypt");
        assert!(is_encrypted(&sealed));
        assert!(!sealed.contains("sk-or-test-123"));
        assert_eq!(decrypt(&sealed).as_deref(), Some("sk-or-test-123"));
        assert_eq!(decrypt("legacy-plain").as_deref(), Some("legacy-plain"));
        assert!(decrypt("enc:v1:zz").is_none());
    }

    #[test]
    fn webhook_tokens_bind_task_and_job() {
        let task = uuid::Uuid::new_v4();
        let token = webhook_token(task, "job_1");
        assert!(verify_webhook_token(task, "job_1", &token));
        assert!(!verify_webhook_token(task, "job_2", &token));
        assert!(!verify_webhook_token(uuid::Uuid::new_v4(), "job_1", &token));
        assert!(!verify_webhook_token(task, "job_1", "not-hex"));
    }

    #[test]
    fn connection_hook_tokens_bind_connection_and_salt() {
        let id = uuid::Uuid::new_v4();
        let token = connection_hook_token(id, "salt-a");
        assert!(verify_connection_hook_token(id, "salt-a", &token));
        assert!(!verify_connection_hook_token(id, "salt-b", &token));
        assert!(!verify_connection_hook_token(
            uuid::Uuid::new_v4(),
            "salt-a",
            &token
        ));
        assert_ne!(token, webhook_token(id, "salt-a"));
    }

    #[test]
    fn pkce_verifiers_are_stable_and_valid() {
        let v = pkce_verifier("abc");
        assert_eq!(v, pkce_verifier("abc"));
        assert_ne!(v, pkce_verifier("abd"));
        assert_eq!(v.len(), 64);
    }
}
