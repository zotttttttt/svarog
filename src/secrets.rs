use crate::config::{CredentialScope, Paths};
use anyhow::{Context, Result};
use keyring::{Entry, Error as KeyringError};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use zeroize::Zeroizing;

const SERVICE: &str = "svarog";
const PRODUCTION_ACCOUNT: &str = "openai-api-key";
const DEVELOPMENT_ACCOUNT: &str = "openai-api-key-development";

pub trait CredentialStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>>;
    fn set(&self, account: &str, secret: &str) -> Result<()>;
    fn delete(&self, account: &str) -> Result<()>;
}

pub struct OsCredentialStore;

#[derive(Default)]
struct CredentialCache {
    entries: Mutex<HashMap<&'static str, Option<Zeroizing<String>>>>,
}

impl CredentialCache {
    fn entries(&self) -> MutexGuard<'_, HashMap<&'static str, Option<Zeroizing<String>>>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn get(
        &self,
        store: &dyn CredentialStore,
        account: &'static str,
    ) -> Result<Option<Zeroizing<String>>> {
        let mut entries = self.entries();
        if let Some(cached) = entries.get(account) {
            return Ok(cached.clone());
        }
        let secret = store.get(account)?;
        entries.insert(account, secret.clone());
        Ok(secret)
    }

    fn save(&self, store: &dyn CredentialStore, account: &'static str, secret: &str) -> Result<()> {
        store.set(account, secret)?;
        self.entries()
            .insert(account, Some(Zeroizing::new(secret.to_string())));
        Ok(())
    }

    fn remove(&self, store: &dyn CredentialStore, account: &'static str) -> Result<()> {
        store.delete(account)?;
        self.entries().insert(account, None);
        Ok(())
    }

    fn clear(&self, account: &'static str) {
        self.entries().remove(account);
    }
}

fn cache() -> &'static CredentialCache {
    static CACHE: OnceLock<CredentialCache> = OnceLock::new();
    CACHE.get_or_init(CredentialCache::default)
}

pub struct OpenAiKeyCacheGuard {
    account: &'static str,
}

impl Drop for OpenAiKeyCacheGuard {
    fn drop(&mut self) {
        cache().clear(self.account);
    }
}

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

pub fn openai_key_cache_guard(paths: &Paths) -> OpenAiKeyCacheGuard {
    OpenAiKeyCacheGuard {
        account: account(paths),
    }
}

pub fn openai_api_key(paths: &Paths) -> Result<Option<Zeroizing<String>>> {
    cache().get(&OsCredentialStore, account(paths))
}

pub fn has_openai_api_key(paths: &Paths) -> Result<bool> {
    Ok(openai_api_key(paths)?.is_some_and(|key| !key.trim().is_empty()))
}

pub fn save_openai_api_key(paths: &Paths, secret: &str) -> Result<()> {
    cache().save(&OsCredentialStore, account(paths), secret)
}

pub fn remove_openai_api_key(paths: &Paths) -> Result<()> {
    cache().remove(&OsCredentialStore, account(paths))
}

pub fn clear_cached_openai_api_key(paths: &Paths) {
    cache().clear(account(paths));
}

pub fn cleanup_openai_api_key(paths: &Paths) -> Option<String> {
    cleanup_with(cache(), &OsCredentialStore, account(paths))
}

fn cleanup_with(
    cache: &CredentialCache,
    store: &dyn CredentialStore,
    account: &'static str,
) -> Option<String> {
    cache
        .remove(store, account)
        .err()
        .map(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct MemoryStore {
        values: Mutex<HashMap<String, String>>,
        reads: AtomicUsize,
    }

    impl CredentialStore for MemoryStore {
        fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .values
                .lock()
                .unwrap()
                .get(account)
                .cloned()
                .map(Zeroizing::new))
        }

        fn set(&self, account: &str, secret: &str) -> Result<()> {
            self.values
                .lock()
                .unwrap()
                .insert(account.to_string(), secret.to_string());
            Ok(())
        }

        fn delete(&self, account: &str) -> Result<()> {
            self.values.lock().unwrap().remove(account);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FailingStore {
        reads: AtomicUsize,
    }

    impl CredentialStore for FailingStore {
        fn get(&self, _account: &str) -> Result<Option<Zeroizing<String>>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
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
    fn credentials_can_be_saved_replaced_and_removed() {
        let store = MemoryStore::default();
        let cache = CredentialCache::default();
        cache.save(&store, PRODUCTION_ACCOUNT, "sk-first").unwrap();
        assert_eq!(
            cache
                .get(&store, PRODUCTION_ACCOUNT)
                .unwrap()
                .unwrap()
                .as_str(),
            "sk-first"
        );

        cache.save(&store, PRODUCTION_ACCOUNT, "sk-second").unwrap();
        assert_eq!(
            cache
                .get(&store, PRODUCTION_ACCOUNT)
                .unwrap()
                .unwrap()
                .as_str(),
            "sk-second"
        );

        cache.remove(&store, PRODUCTION_ACCOUNT).unwrap();
        assert!(cache.get(&store, PRODUCTION_ACCOUNT).unwrap().is_none());
        assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn concurrent_reads_share_one_credential_store_lookup() {
        let store = MemoryStore::default();
        store.set(PRODUCTION_ACCOUNT, "sk-shared").unwrap();
        let cache = CredentialCache::default();

        std::thread::scope(|scope| {
            let handles = (0..8)
                .map(|_| scope.spawn(|| cache.get(&store, PRODUCTION_ACCOUNT).unwrap().unwrap()))
                .collect::<Vec<_>>();
            for handle in handles {
                assert_eq!(handle.join().unwrap().as_str(), "sk-shared");
            }
        });

        assert_eq!(store.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn clearing_cache_forces_one_new_lookup() {
        let store = MemoryStore::default();
        store.set(PRODUCTION_ACCOUNT, "sk-shared").unwrap();
        let cache = CredentialCache::default();

        assert!(cache.get(&store, PRODUCTION_ACCOUNT).unwrap().is_some());
        assert!(cache.get(&store, PRODUCTION_ACCOUNT).unwrap().is_some());
        assert_eq!(store.reads.load(Ordering::SeqCst), 1);

        cache.clear(PRODUCTION_ACCOUNT);
        assert!(cache.get(&store, PRODUCTION_ACCOUNT).unwrap().is_some());
        assert_eq!(store.reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn missing_credentials_are_cached_but_access_errors_are_retryable() {
        let missing = MemoryStore::default();
        let missing_cache = CredentialCache::default();
        assert!(missing_cache
            .get(&missing, PRODUCTION_ACCOUNT)
            .unwrap()
            .is_none());
        assert!(missing_cache
            .get(&missing, PRODUCTION_ACCOUNT)
            .unwrap()
            .is_none());
        assert_eq!(missing.reads.load(Ordering::SeqCst), 1);

        let failing = FailingStore::default();
        let failing_cache = CredentialCache::default();
        assert!(failing_cache.get(&failing, PRODUCTION_ACCOUNT).is_err());
        assert!(failing_cache.get(&failing, PRODUCTION_ACCOUNT).is_err());
        assert_eq!(failing.reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn production_and_development_use_different_accounts() {
        let root = tempfile::tempdir().unwrap();
        let development = Paths::from_root(root.path().to_path_buf());
        let mut production = development.clone();
        production.credential_scope = CredentialScope::Production;

        assert_ne!(account(&production), account(&development));

        let store = MemoryStore::default();
        let cache = CredentialCache::default();
        store.set(account(&production), "sk-production").unwrap();
        store.set(account(&development), "sk-development").unwrap();
        assert!(cleanup_with(&cache, &store, account(&production)).is_none());
        assert!(store.get(account(&production)).unwrap().is_none());
        assert_eq!(
            store.get(account(&development)).unwrap().unwrap().as_str(),
            "sk-development"
        );
    }

    #[test]
    fn reset_cleanup_reports_store_failures_without_weakening_strict_deletion() {
        let cache = CredentialCache::default();
        let store = FailingStore::default();
        let warning = cleanup_with(&cache, &store, PRODUCTION_ACCOUNT).unwrap();
        assert_eq!(warning, "credential store is locked");

        let strict = cache.remove(&store, PRODUCTION_ACCOUNT);
        assert!(strict.is_err());
    }

    #[test]
    fn reset_cleanup_treats_deleted_and_missing_credentials_as_success() {
        let store = MemoryStore::default();
        let cache = CredentialCache::default();
        assert!(cleanup_with(&cache, &store, PRODUCTION_ACCOUNT).is_none());

        store.set(DEVELOPMENT_ACCOUNT, "sk-development").unwrap();
        assert!(cleanup_with(&cache, &store, DEVELOPMENT_ACCOUNT).is_none());
        assert!(store.get(DEVELOPMENT_ACCOUNT).unwrap().is_none());
    }
}
