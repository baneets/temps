//! Per-workspace-session preview password.
//!
//! Each workspace session gets a random password when it's created. The
//! plaintext is returned to the caller **once** (shown in the UI with a
//! show-once pattern, same as API tokens) and is then only stored as an
//! argon2 hash on the session row. The host-side Pingora verifies
//! incoming Basic Auth credentials against this hash before forwarding
//! preview traffic to the preview gateway.
//!
//! This module is deliberately tiny — generation, hashing, and verification
//! only. Persistence lives in `WorkspaceService`.

use argon2::password_hash::{rand_core::OsRng, PasswordHash, SaltString};
use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use rand::{rngs::OsRng as RandOsRng, RngCore};

/// Number of characters in generated preview passwords. 24 characters
/// from our 32-char alphabet = 120 bits of entropy, comfortably above
/// the ~80 bits you'd want even against a well-funded offline attacker
/// (and well above what a Pingora rate limiter will ever let through).
const PASSWORD_LEN: usize = 24;

/// Base32-ish alphabet without confusable characters (`0`, `O`, `I`, `l`,
/// `1`). Lowercase for easier manual entry.
const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Plaintext password returned to the caller exactly once, plus the
/// artifacts that need to be persisted alongside it.
pub struct GeneratedPassword {
    /// The plaintext. Show to the user once, then discard.
    pub plaintext: String,
    /// Argon2 PHC string. Persist in `preview_password_hash`.
    pub hash: String,
    /// Last 4 characters of the plaintext. Safe to display in the UI
    /// after the one-time reveal — helps users tell two passwords
    /// apart without exposing enough to be useful to an attacker.
    pub hint: String,
}

/// Generate a fresh random password and its argon2 hash.
pub fn generate() -> Result<GeneratedPassword, String> {
    let mut rng = RandOsRng;
    let plaintext: String = (0..PASSWORD_LEN)
        .map(|_| {
            let mut b = [0u8; 1];
            rng.fill_bytes(&mut b);
            ALPHABET[(b[0] as usize) % ALPHABET.len()] as char
        })
        .collect();

    let hash = hash_password(&plaintext)?;
    let hint = last_four(&plaintext);

    Ok(GeneratedPassword {
        plaintext,
        hash,
        hint,
    })
}

/// Hash an arbitrary plaintext password. Used by `generate` and by the
/// regenerate flow when we accept a user-supplied password (future).
pub fn hash_password(plaintext: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plaintext.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("argon2 hash failed: {}", e))
}

/// Verify a plaintext attempt against a stored argon2 PHC hash.
/// Returns `Ok(true)` on match, `Ok(false)` on mismatch, `Err` only if
/// the hash string itself is malformed.
pub fn verify_password(attempt: &str, stored_hash: &str) -> Result<bool, String> {
    let parsed =
        PasswordHash::new(stored_hash).map_err(|e| format!("stored hash is invalid: {}", e))?;
    Ok(Argon2::default()
        .verify_password(attempt.as_bytes(), &parsed)
        .is_ok())
}

fn last_four(s: &str) -> String {
    let count = s.chars().count();
    s.chars().skip(count.saturating_sub(4)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_produces_expected_shape() {
        let gp = generate().expect("generate");
        assert_eq!(gp.plaintext.chars().count(), PASSWORD_LEN);
        assert_eq!(gp.hint.chars().count(), 4);
        assert!(gp.hash.starts_with("$argon2"));
        // Hint is the tail of the plaintext.
        assert!(gp.plaintext.ends_with(&gp.hint));
        // Alphabet check — all chars must come from our safe alphabet.
        for c in gp.plaintext.chars() {
            assert!(
                ALPHABET.contains(&(c as u8)),
                "character {:?} not in alphabet",
                c
            );
        }
    }

    #[test]
    fn verify_round_trip() {
        let gp = generate().expect("generate");
        assert!(verify_password(&gp.plaintext, &gp.hash).unwrap());
        assert!(!verify_password("wrong-password-000000000", &gp.hash).unwrap());
    }

    #[test]
    fn two_generations_differ() {
        let a = generate().unwrap();
        let b = generate().unwrap();
        assert_ne!(a.plaintext, b.plaintext);
        assert_ne!(a.hash, b.hash);
    }

    #[test]
    fn malformed_stored_hash_errors() {
        let err = verify_password("anything", "not-a-phc-string");
        assert!(err.is_err());
    }
}
