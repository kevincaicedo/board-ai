//! Secrets are referenced by name in settings and resolved through a [`SecretStore`] (LLD §3.8).
//! The Keychain store lives in the app crate; this module has the seam and an in-memory store.
//!
//! A [`Secret`] never prints its value: `Debug` is redacted and there is no `Display`, so a
//! secret cannot reach a log or an error message by accident (law 10).

use std::collections::BTreeMap;
use std::fmt;

use crate::limits::SECRET_NAME_MAX_BYTES;

/// The name a secret is stored under, for example `anthropic`. 1 to [`SECRET_NAME_MAX_BYTES`]
/// bytes of lowercase ASCII letters, digits and `. _ -`, so a pasted key (mixed case, long) is
/// rejected as a name instead of being treated as one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretName(String);

impl SecretName {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let valid_byte = |byte: u8| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        };
        let valid =
            !text.is_empty() && text.len() <= SECRET_NAME_MAX_BYTES && text.bytes().all(valid_byte);
        valid.then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A secret value. Read it only where it goes on the wire, with [`Secret::expose`].
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(<redacted>)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretError {
    /// Nothing is stored under the name; the first-run screen asks for it.
    Missing { name: SecretName },
    /// The store could not be read (a locked Keychain, a denied prompt).
    Unavailable { name: SecretName, detail: String },
}

impl fmt::Display for SecretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { name } => write!(formatter, "no secret is stored as `{name}`"),
            Self::Unavailable { name, detail } => {
                write!(formatter, "the secret `{name}` could not be read: {detail}")
            }
        }
    }
}

impl std::error::Error for SecretError {}

/// Where secrets live. Reads may block (the Keychain can prompt), so async callers use the
/// blocking pool.
pub trait SecretStore: Send + Sync {
    fn get(&self, name: &SecretName) -> Result<Secret, SecretError>;
}

/// A store for tests and the conversation simulator.
#[derive(Clone, Debug, Default)]
pub struct MemorySecretStore {
    secrets: BTreeMap<SecretName, Secret>,
}

impl MemorySecretStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: SecretName, secret: Secret) {
        self.secrets.insert(name, secret);
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, name: &SecretName) -> Result<Secret, SecretError> {
        self.secrets
            .get(name)
            .cloned()
            .ok_or_else(|| SecretError::Missing { name: name.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::{MemorySecretStore, Secret, SecretError, SecretName, SecretStore};

    #[test]
    fn names_are_parsed_and_pasted_keys_are_not_names() {
        assert!(SecretName::parse("anthropic").is_some());
        assert!(SecretName::parse("eleven_labs-2.key").is_some());
        for bad in [
            "",
            "Anthropic",
            "sk-ant-api03-AbCdEf",
            "has space",
            &"a".repeat(65),
        ] {
            assert!(SecretName::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_secret_never_prints_its_value() {
        let secret = Secret::new("sk-ant-very-secret".to_owned());
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        let mut store = MemorySecretStore::new();
        let name = SecretName::parse("anthropic");
        let Some(name) = name else {
            panic!("valid name")
        };
        store.insert(name.clone(), secret);
        assert_eq!(
            store.get(&name).map(|found| found.expose().len()),
            Ok("sk-ant-very-secret".len())
        );
        let missing = SecretName::parse("openrouter").map(|other| store.get(&other));
        assert!(matches!(missing, Some(Err(SecretError::Missing { .. }))));
    }
}
