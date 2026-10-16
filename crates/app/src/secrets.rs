//! Secrets from the macOS Keychain (LLD §3.8): generic passwords under the service
//! `ai.boardai.desktop`, one account per secret name (`anthropic`, `elevenlabs`).
//!
//! Reads and writes can block (the Keychain may ask the user), so callers use the blocking
//! pool.
//! Error details come from the Keychain's `Display`, which never prints the secret's bytes.

use apple_native_keyring_store::keychain::{Cred, MacKeychainDomain};
use boardai_runtime::logging::{bound, redact};
use boardai_runtime::settings::{Secret, SecretError, SecretName, SecretStore};

/// The Keychain service every BoardAI secret is stored under.
pub const KEYCHAIN_SERVICE: &str = "ai.boardai.desktop";

/// Bytes of a Keychain error kept in a secret error's detail.
const KEYCHAIN_ERROR_DETAIL_MAX_BYTES: usize = 200;

#[derive(Clone, Copy, Debug, Default)]
pub struct KeychainSecrets;

impl SecretStore for KeychainSecrets {
    fn get(&self, name: &SecretName) -> Result<Secret, SecretError> {
        let unavailable = |error: &keyring_core::Error| SecretError::Unavailable {
            name: name.clone(),
            detail: detail(error),
        };
        let entry = Cred::build(MacKeychainDomain::User, KEYCHAIN_SERVICE, name.as_str())
            .map_err(|error| unavailable(&error))?;
        match entry.get_password() {
            Ok(value) => Ok(Secret::new(value)),
            Err(keyring_core::Error::NoEntry) => Err(SecretError::Missing { name: name.clone() }),
            Err(error) => Err(unavailable(&error)),
        }
    }

    /// Creates or replaces the generic password (the first-run screen's key field).
    fn set(&self, name: &SecretName, secret: &Secret) -> Result<(), SecretError> {
        let unwritable = |error: &keyring_core::Error| SecretError::Unwritable {
            name: name.clone(),
            detail: detail(error),
        };
        let entry = Cred::build(MacKeychainDomain::User, KEYCHAIN_SERVICE, name.as_str())
            .map_err(|error| unwritable(&error))?;
        entry
            .set_password(secret.expose())
            .map_err(|error| unwritable(&error))
    }
}

/// A Keychain error for a log line or a secret error: redacted and bounded.
fn detail(error: &keyring_core::Error) -> String {
    let mut detail = redact(&error.to_string()).into_owned();
    bound(&mut detail, KEYCHAIN_ERROR_DETAIL_MAX_BYTES);
    detail
}
