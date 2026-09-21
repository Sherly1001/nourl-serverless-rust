use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// User id (the `users.id` uuid string, not the Mongo `_id`).
    pub sub: String,
    /// Snapshot of the user's `token_version` when the token was minted.
    pub ver: i64,
    pub exp: i64,
}

/// The cookie's `Max-Age` must come from the same `session_days`, or the
/// browser keeps sending a token the server already rejects.
pub fn encode(
    secret: &str,
    user_id: &str,
    token_version: i64,
    session_days: i64,
) -> Result<String, AppError> {
    let claims = Claims {
        sub: user_id.to_string(),
        ver: token_version,
        exp: (chrono::Utc::now() + chrono::Duration::days(session_days)).timestamp(),
    };
    jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(AppError::internal)
}

/// Every failure is a 401; the reason is not echoed back.
pub fn decode(secret: &str, token: &str) -> Result<Claims, AppError> {
    jsonwebtoken::decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .map(|data| data.claims)
    .map_err(|_| AppError::unauthorized("invalid or expired session"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret";

    #[test]
    fn round_trips_subject_and_version() {
        let token = encode(SECRET, "user-1", 7, 60).unwrap();
        let claims = decode(SECRET, &token).unwrap();
        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.ver, 7);
    }

    #[test]
    fn rejects_a_token_signed_with_another_secret() {
        let token = encode("other-secret", "user-1", 0, 60).unwrap();
        assert!(decode(SECRET, &token).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(SECRET, "not.a.token").is_err());
        assert!(decode(SECRET, "").is_err());
    }

    #[test]
    fn expiry_follows_the_configured_lifetime() {
        for configured in [1, 7, 60] {
            let token = encode(SECRET, "user-1", 0, configured).unwrap();
            let claims = decode(SECRET, &token).unwrap();
            let days = (claims.exp - chrono::Utc::now().timestamp()) / 86_400;
            // One less only if a whole second elapsed between mint and read.
            assert!(
                ((configured - 1)..=configured).contains(&days),
                "expiry was {days} days out, expected {configured}"
            );
        }
    }
}
