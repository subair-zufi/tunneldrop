use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

/// Hashes a plaintext password using argon2. Returns the encoded hash string.
pub fn hash_password(plain: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .expect("hashing should not fail")
        .to_string()
}

/// Verifies a plaintext password against an encoded argon2 hash.
pub fn verify_password(plain: &str, encoded: &str) -> bool {
    match PasswordHash::new(encoded) {
        Ok(parsed) => Argon2::default()
            .verify_password(plain.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correct_password_verifies() {
        let hash = hash_password("hunter2");
        assert!(verify_password("hunter2", &hash));
    }

    #[test]
    fn wrong_password_fails() {
        let hash = hash_password("hunter2");
        assert!(!verify_password("nope", &hash));
    }

    #[test]
    fn hash_is_not_plaintext() {
        let hash = hash_password("hunter2");
        assert!(!hash.contains("hunter2"));
    }
}
