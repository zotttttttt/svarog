use crate::config::{CredentialScope, Paths};
use anyhow::{Context, Result};
use keyring::{Entry, Error as KeyringError};
use std::fmt;
use zeroize::Zeroizing;

const SERVICE: &str = "svarog";
const PRODUCTION_ACCOUNT: &str = "openai-api-key";
const DEVELOPMENT_ACCOUNT: &str = "openai-api-key-development";

#[derive(Clone)]
pub enum PendingSecretChange {
    Set(Zeroizing<String>),
    Delete,
}

impl fmt::Debug for PendingSecretChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Set(_) => formatter.write_str("Set([REDACTED])"),
            Self::Delete => formatter.write_str("Delete"),
        }
    }
}

pub trait CredentialStore {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>>;
    fn set(&self, account: &str, secret: &str) -> Result<()>;
    fn delete(&self, account: &str) -> Result<()>;
}

pub struct OsCredentialStore;

impl OsCredentialStore {
    fn entry(account: &str) -> Result<Entry> {
        Entry::new(SERVICE, account).context("opening the operating system credential store")
    }
}

impl CredentialStore for OsCredentialStore {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>> {
        match Self::entry(account)?.get_password() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(error) => Err(error).context("reading the saved OpenAI API key"),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<()> {
        Self::entry(account)?
            .set_password(secret)
            .context("saving the OpenAI API key in the operating system credential store")
    }

    fn delete(&self, account: &str) -> Result<()> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(error) => Err(error).context("removing the saved OpenAI API key"),
        }
    }
}

pub fn account(paths: &Paths) -> &'static str {
    match paths.credential_scope {
        CredentialScope::Production => PRODUCTION_ACCOUNT,
        CredentialScope::Development => DEVELOPMENT_ACCOUNT,
    }
}

pub fn openai_api_key(paths: &Paths) -> Result<Option<Zeroizing<String>>> {
    OsCredentialStore.get(account(paths))
}

pub fn has_openai_api_key(paths: &Paths) -> Result<bool> {
    Ok(openai_api_key(paths)?.is_some_and(|key| !key.trim().is_empty()))
}

pub fn apply_openai_api_key_change(paths: &Paths, change: &PendingSecretChange) -> Result<()> {
    apply_change(&OsCredentialStore, account(paths), change)
}

pub fn cleanup_openai_api_key(paths: &Paths) -> Option<String> {
    cleanup_with(&OsCredentialStore, account(paths))
}

fn apply_change(
    store: &dyn CredentialStore,
    account: &str,
    change: &PendingSecretChange,
) -> Result<()> {
    match change {
        PendingSecretChange::Set(secret) => store.set(account, secret),
        PendingSecretChange::Delete => store.delete(account),
    }
}

fn cleanup_with(store: &dyn CredentialStore, account: &str) -> Option<String> {
    store.delete(account).err().map(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryStore(Mutex<HashMap<String, String>>);

    impl CredentialStore for MemoryStore {
        fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(account)
                .cloned()
                .map(Zeroizing::new))
        }

        fn set(&self, account: &str, secret: &str) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(account.to_string(), secret.to_string());
            Ok(())
        }

        fn delete(&self, account: &str) -> Result<()> {
            self.0.lock().unwrap().remove(account);
            Ok(())
        }
    }

    struct FailingStore;

    impl CredentialStore for FailingStore {
        fn get(&self, _account: &str) -> Result<Option<Zeroizing<String>>> {
            anyhow::bail!("credential store is locked")
        }

        fn set(&self, _account: &str, _secret: &str) -> Result<()> {
            anyhow::bail!("credential store is locked")
        }

        fn delete(&self, _account: &str) -> Result<()> {
            anyhow::bail!("credential store is locked")
        }
    }

    #[test]
    fn staged_changes_set_replace_and_delete_without_exposing_debug_values() {
        let store = MemoryStore::default();
        let first = PendingSecretChange::Set(Zeroizing::new("sk-first".to_string()));
        assert_eq!(format!("{first:?}"), "Set([REDACTED])");
        apply_change(&store, PRODUCTION_ACCOUNT, &first).unwrap();
        assert_eq!(
            store.get(PRODUCTION_ACCOUNT).unwrap().unwrap().as_str(),
            "sk-first"
        );

        let replacement = PendingSecretChange::Set(Zeroizing::new("sk-second".to_string()));
        apply_change(&store, PRODUCTION_ACCOUNT, &replacement).unwrap();
        assert_eq!(
            store.get(PRODUCTION_ACCOUNT).unwrap().unwrap().as_str(),
            "sk-second"
        );

        apply_change(&store, PRODUCTION_ACCOUNT, &PendingSecretChange::Delete).unwrap();
        assert!(store.get(PRODUCTION_ACCOUNT).unwrap().is_none());
    }

    #[test]
    fn production_and_development_use_different_accounts() {
        let root = tempfile::tempdir().unwrap();
        let development = Paths::from_root(root.path().to_path_buf());
        let mut production = development.clone();
        production.credential_scope = CredentialScope::Production;

        assert_ne!(account(&production), account(&development));

        let store = MemoryStore::default();
        store.set(account(&production), "sk-production").unwrap();
        store.set(account(&development), "sk-development").unwrap();
        assert!(cleanup_with(&store, account(&production)).is_none());
        assert!(store.get(account(&production)).unwrap().is_none());
        assert_eq!(
            store.get(account(&development)).unwrap().unwrap().as_str(),
            "sk-development"
        );
    }

    #[test]
    fn reset_cleanup_reports_store_failures_without_weakening_strict_deletion() {
        let warning = cleanup_with(&FailingStore, PRODUCTION_ACCOUNT).unwrap();
        assert_eq!(warning, "credential store is locked");

        let strict = apply_change(
            &FailingStore,
            PRODUCTION_ACCOUNT,
            &PendingSecretChange::Delete,
        );
        assert!(strict.is_err());
    }

    #[test]
    fn reset_cleanup_treats_deleted_and_missing_credentials_as_success() {
        let store = MemoryStore::default();
        assert!(cleanup_with(&store, PRODUCTION_ACCOUNT).is_none());

        store.set(DEVELOPMENT_ACCOUNT, "sk-development").unwrap();
        assert!(cleanup_with(&store, DEVELOPMENT_ACCOUNT).is_none());
        assert!(store.get(DEVELOPMENT_ACCOUNT).unwrap().is_none());
    }
}
