//! Secrets from the macOS Keychain (LLD §3.8): generic passwords under the service
//! `ai.boardai.desktop`, one account per secret name (`anthropic`, `elevenlabs`).
//!
//! Reads can block (the Keychain may ask the user), so the session reads on the blocking pool.
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
        let unavailable = |error: &keyring_core::Error| {
            let mut detail = redact(&error.to_string()).into_owned();
            bound(&mut detail, KEYCHAIN_ERROR_DETAIL_MAX_BYTES);
            SecretError::Unavailable {
                name: name.clone(),
                detail,
            }
        };
        let entry = Cred::build(MacKeychainDomain::User, KEYCHAIN_SERVICE, name.as_str())
            .map_err(|error| unavailable(&error))?;
        match entry.get_password() {
            Ok(value) => Ok(Secret::new(value)),
            Err(keyring_core::Error::NoEntry) => Err(SecretError::Missing { name: name.clone() }),
            Err(error) => Err(unavailable(&error)),
        }
    }
}
