use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

use crate::error::AppError;

/// argon2id as a PHC string: parameters and salt travel with the hash, so
/// they can change without invalidating stored ones.
pub fn hash(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(AppError::internal)
}

/// False for an unparseable stored hash too: corruption is not a bypass.
pub fn verify(password: &str, stored: &str) -> bool {
    PasswordHash::new(stored).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_verifies_against_its_own_password() {
        let stored = hash("correct horse battery").unwrap();
        assert!(verify("correct horse battery", &stored));
        assert!(!verify("wrong password", &stored));
    }

    #[test]
    fn hashes_are_salted_so_equal_passwords_differ() {
        let a = hash("same-password").unwrap();
        let b = hash("same-password").unwrap();
        assert_ne!(a, b);
        assert!(verify("same-password", &a));
        assert!(verify("same-password", &b));
    }

    #[test]
    fn garbage_stored_hash_is_rejected_not_panicked() {
        assert!(!verify("anything", "not-a-phc-string"));
    }
}
