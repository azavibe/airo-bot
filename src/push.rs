use std::time::Duration;

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;
use web_push_native::jwt_simple::algorithms::ES256KeyPair;
use web_push_native::p256::elliptic_curve::sec1::ToEncodedPoint;
use web_push_native::{Auth, WebPushBuilder};

use crate::config::VapidConfig;
use crate::models::PushSubscription;

struct ConfiguredPush {
    key_pair: ES256KeyPair,
    client: reqwest::Client,
    public_key: String,
    subject: String,
}

pub struct PushSender {
    configured: Option<ConfiguredPush>,
}

#[derive(Serialize)]
struct PushPayload<'a> {
    title: &'a str,
    body: &'a str,
    href: &'a str,
}

fn base64_url_no_pad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        encoded.push(ALPHABET[(a >> 2) as usize] as char);
        encoded.push(ALPHABET[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[(c & 0x3f) as usize] as char);
        }
    }
    encoded
}

fn decode_base64_url(value: &str) -> Result<Vec<u8>, String> {
    fn sextet(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'-' | b'+' => Some(62),
            b'_' | b'/' => Some(63),
            _ => None,
        }
    }

    let input = value.trim().trim_end_matches('=');
    if input.len() % 4 == 1 {
        return Err("invalid base64url length".to_string());
    }
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut accumulator = 0_u32;
    let mut bits = 0_u8;
    for byte in input.bytes() {
        let value = sextet(byte).ok_or_else(|| "invalid base64url character".to_string())?;
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
            accumulator &= (1_u32 << bits) - 1;
        }
    }
    Ok(output)
}

impl PushSender {
    pub fn new(config: Option<&VapidConfig>) -> Result<Self, String> {
        let configured = match config {
            Some(config) => {
                let private_key = decode_base64_url(&config.private_key)?;
                if private_key.len() != 32 {
                    return Err("VAPID_PRIVATE_KEY must decode to 32 bytes".to_string());
                }
                let key_pair = ES256KeyPair::from_bytes(&private_key)
                    .map_err(|error| format!("invalid VAPID private key: {error}"))?;
                let secret_key = web_push_native::p256::SecretKey::from_slice(&private_key)
                    .map_err(|_| "invalid VAPID private key".to_string())?;
                let public_key =
                    base64_url_no_pad(secret_key.public_key().to_encoded_point(false).as_bytes());
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(10))
                    .build()
                    .map_err(|error| format!("failed to initialize Web Push client: {error}"))?;
                Some(ConfiguredPush {
                    key_pair,
                    client,
                    public_key,
                    subject: config.subject.clone(),
                })
            }
            None => None,
        };
        Ok(Self { configured })
    }

    pub fn public_key(&self) -> Option<&str> {
        self.configured
            .as_ref()
            .map(|configured| configured.public_key.as_str())
    }

    pub async fn send_to_user(
        &self,
        pool: &PgPool,
        user_id: Uuid,
        title: &str,
        body: &str,
        href: &str,
    ) -> bool {
        let Some(configured) = &self.configured else {
            return false;
        };
        let subscriptions = match sqlx::query_as::<_, PushSubscription>(
            "SELECT * FROM push_subscriptions
             WHERE user_id = $1 AND deleted_at IS NULL",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await
        {
            Ok(subscriptions) => subscriptions,
            Err(error) => {
                tracing::error!(%user_id, %error, "failed to load Web Push subscriptions");
                return false;
            }
        };
        let payload = match serde_json::to_vec(&PushPayload { title, body, href }) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::error!(%user_id, %error, "failed to encode Web Push payload");
                return false;
            }
        };
        let mut delivered = false;

        for subscription in subscriptions {
            let endpoint = match subscription.endpoint.parse() {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    tracing::warn!(subscription_id = %subscription.id, %error, "invalid stored Web Push endpoint");
                    continue;
                }
            };
            let public_key = match decode_base64_url(&subscription.p256dh)
                .ok()
                .and_then(|bytes| web_push_native::p256::PublicKey::from_sec1_bytes(&bytes).ok())
            {
                Some(public_key) => public_key,
                None => {
                    tracing::warn!(subscription_id = %subscription.id, "invalid stored Web Push public key");
                    continue;
                }
            };
            let auth = match decode_base64_url(&subscription.auth) {
                Ok(auth) if auth.len() == 16 => Auth::clone_from_slice(&auth),
                _ => {
                    tracing::warn!(subscription_id = %subscription.id, "invalid stored Web Push auth secret");
                    continue;
                }
            };
            let request = match WebPushBuilder::new(endpoint, public_key, auth)
                .with_vapid(&configured.key_pair, &configured.subject)
                .build(payload.clone())
            {
                Ok(request) => request,
                Err(error) => {
                    tracing::warn!(subscription_id = %subscription.id, %error, "failed to build Web Push request");
                    continue;
                }
            };
            let (parts, body) = request.into_parts();
            let response = configured
                .client
                .request(parts.method, parts.uri.to_string())
                .headers(parts.headers)
                .body(body)
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => delivered = true,
                Ok(response)
                    if response.status() == reqwest::StatusCode::NOT_FOUND
                        || response.status() == reqwest::StatusCode::GONE =>
                {
                    tracing::info!(subscription_id = %subscription.id, status = %response.status(), "retiring expired Web Push subscription");
                    if let Err(error) = sqlx::query(
                        "UPDATE push_subscriptions SET deleted_at = now(), updated_at = now()
                         WHERE id = $1 AND deleted_at IS NULL",
                    )
                    .bind(subscription.id)
                    .execute(pool)
                    .await
                    {
                        tracing::error!(subscription_id = %subscription.id, %error, "failed to retire Web Push subscription");
                    }
                }
                Ok(response) => {
                    tracing::warn!(subscription_id = %subscription.id, status = %response.status(), "Web Push delivery failed");
                }
                Err(error) => {
                    tracing::warn!(subscription_id = %subscription.id, %error, "Web Push delivery failed");
                }
            }
        }
        delivered
    }
}

#[cfg(test)]
mod tests {
    use super::{base64_url_no_pad, decode_base64_url};

    #[test]
    fn base64_url_roundtrip_has_no_padding() {
        for bytes in [b"".as_slice(), b"f", b"fo", b"foo", &[0xfb, 0xff]] {
            let encoded = base64_url_no_pad(bytes);
            assert!(!encoded.contains('='));
            assert_eq!(decode_base64_url(&encoded).expect("decode"), bytes);
        }
    }
}
