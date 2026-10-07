//! API keys live in the operating system's credential store, never in the config file.
//!
//! Tests use an in-memory store instead, so they never touch anyone's real keychain and also run
//! on machines (such as headless CI runners) that have no credential store at all.

pub(crate) use backend::{delete, load, store};

#[cfg(not(test))]
mod backend {
    use anyhow::{Context, Result};

    const SERVICE: &str = "harness";

    pub(crate) fn store(provider_id: &str, api_key: &str) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE, provider_id)
            .context("opening OS credential-store entry")?;
        entry
            .set_password(api_key)
            .context("saving API key to OS credential store")
    }

    pub(crate) fn load(provider_id: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(SERVICE, provider_id)
            .context("opening OS credential-store entry")?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error).context("reading API key from OS credential store"),
        }
    }

    pub(crate) fn delete(provider_id: &str) -> Result<()> {
        let entry = keyring::Entry::new(SERVICE, provider_id)
            .context("opening OS credential-store entry")?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error).context("deleting API key from OS credential store"),
        }
    }
}

#[cfg(test)]
mod backend {
    use anyhow::Result;
    use std::collections::HashMap;
    use std::sync::Mutex;

    static KEYS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

    fn with_keys<T>(action: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
        let mut guard = KEYS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        action(guard.get_or_insert_with(HashMap::new))
    }

    pub(crate) fn store(provider_id: &str, api_key: &str) -> Result<()> {
        with_keys(|keys| keys.insert(provider_id.to_owned(), api_key.to_owned()));
        Ok(())
    }

    pub(crate) fn load(provider_id: &str) -> Result<Option<String>> {
        Ok(with_keys(|keys| keys.get(provider_id).cloned()))
    }

    pub(crate) fn delete(provider_id: &str) -> Result<()> {
        with_keys(|keys| keys.remove(provider_id));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{delete, load, store};

    #[test]
    fn a_stored_key_can_be_read_back_and_deleted() {
        store("secrets-test-a", "key-1").expect("store");
        assert_eq!(load("secrets-test-a").unwrap().as_deref(), Some("key-1"));
        store("secrets-test-a", "key-2").expect("overwrite");
        assert_eq!(load("secrets-test-a").unwrap().as_deref(), Some("key-2"));
        delete("secrets-test-a").expect("delete");
        assert_eq!(load("secrets-test-a").unwrap(), None);
    }

    #[test]
    fn missing_keys_and_double_deletes_are_not_errors() {
        assert_eq!(load("secrets-test-never-stored").unwrap(), None);
        delete("secrets-test-never-stored").expect("delete nothing");
        delete("secrets-test-never-stored").expect("and again");
    }
}
