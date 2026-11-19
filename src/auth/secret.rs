use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::Rng;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Every secret Air issues is a 256-bit random token, never a human
/// password, so Argon2's memory hardness protects nothing an attacker could
/// brute-force. The defaults (19 MiB, t=2) cost several milliseconds and
/// 19 MiB per authenticated request; these parameters keep Argon2id (the
/// standing rule) at roughly a millisecond. Existing hashes keep verifying
/// because the PHC string records the parameters it was made with.
fn hasher() -> Argon2<'static> {
    let params = Params::new(4 * 1024, 1, 1, None).unwrap_or_default();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Argon2-hash a secret (session token, agent key) before it's stored, per
/// the standing "all secrets as argon2 hashes" rule.
pub fn hash_secret(secret: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(hasher()
        .hash_password(secret.as_bytes(), &salt)?
        .to_string())
}

pub fn verify_secret(secret: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    hasher().verify_password(secret.as_bytes(), &parsed).is_ok()
}

const VERIFIED_TTL: Duration = Duration::from_secs(300);
const VERIFIED_MAX_ENTRIES: usize = 10_000;

type VerifiedMap = HashMap<String, ([u8; 32], Instant)>;

fn verified() -> &'static Mutex<VerifiedMap> {
    static VERIFIED: OnceLock<Mutex<VerifiedMap>> = OnceLock::new();
    VERIFIED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn digest(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

fn same_digest(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// `verify_secret` for bearer credentials checked on every request
/// (sessions, agent keys). A successful Argon2 verification is remembered
/// in process for five minutes, keyed by the stored hash, so polling agents
/// don't pay for Argon2 on every call. Callers still load the row with its
/// `revoked_at`/`expires_at` filters on every request, so revocation stays
/// immediate; only the CPU-bound comparison is skipped.
/// TODO(air): in-process memo, not a cache layer -- a second replica would
/// simply re-verify once; revisit only if Air ever runs more than one.
pub fn verify_secret_memoized(secret: &str, hash: &str) -> bool {
    let wanted = digest(secret);
    if let Ok(map) = verified().lock() {
        if let Some((known, at)) = map.get(hash) {
            if at.elapsed() < VERIFIED_TTL {
                return same_digest(known, &wanted);
            }
        }
    }
    if !verify_secret(secret, hash) {
        return false;
    }
    if let Ok(mut map) = verified().lock() {
        if map.len() >= VERIFIED_MAX_ENTRIES {
            map.retain(|_, (_, at)| at.elapsed() < VERIFIED_TTL);
            if map.len() >= VERIFIED_MAX_ENTRIES {
                map.clear();
            }
        }
        map.insert(hash.to_string(), (wanted, Instant::now()));
    }
    true
}

/// A high-entropy random token, hex-encoded.
pub fn random_hex_token(byte_len: usize) -> String {
    let mut bytes = vec![0u8; byte_len];
    rand::thread_rng().fill(bytes.as_mut_slice());
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Argon2 hashes aren't deterministic (random salt), so a secret can't be
/// looked up by hashing it and comparing -- the row has to be found first.
/// Every bearer secret we issue therefore embeds the id of the row it
/// belongs to, so the lookup is a plain primary-key `SELECT` and only the
/// hash comparison needs argon2::verify.
pub fn encode_keyed_secret(prefix: &str, id: Uuid, secret: &str) -> String {
    format!("{prefix}{id}.{secret}")
}

pub fn decode_keyed_secret(prefix: &str, raw: &str) -> Option<(Uuid, String)> {
    let rest = raw.strip_prefix(prefix)?;
    let (id_part, secret_part) = rest.split_once('.')?;
    let id = Uuid::parse_str(id_part).ok()?;
    Some((id, secret_part.to_string()))
}

const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // no 0/O/1/I

/// A short, human-typeable code from an alphabet with no easily-confused
/// characters, for the device flow's login code.
pub fn random_code(len: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let hash = hash_secret("correct horse battery staple").expect("hash");
        assert!(verify_secret("correct horse battery staple", &hash));
        assert!(!verify_secret("wrong", &hash));
    }

    #[test]
    fn memoized_verify_rejects_wrong_secret_after_success() {
        let hash = hash_secret("right-secret").expect("hash");
        assert!(verify_secret_memoized("right-secret", &hash));
        assert!(verify_secret_memoized("right-secret", &hash));
        assert!(!verify_secret_memoized("wrong-secret", &hash));
    }

    #[test]
    fn keyed_secret_roundtrip() {
        let id = Uuid::new_v4();
        let encoded = encode_keyed_secret("air_ag_", id, "abc123");
        let (decoded_id, secret) = decode_keyed_secret("air_ag_", &encoded).expect("decode");
        assert_eq!(decoded_id, id);
        assert_eq!(secret, "abc123");
    }

    #[test]
    fn decode_rejects_wrong_prefix() {
        assert!(decode_keyed_secret("air_ag_", "nope.abc").is_none());
    }
}
