use anyhow::{Context, Result};

const SERVICE: &str = "harness";

pub(crate) fn store(provider_id: &str, api_key: &str) -> Result<()> {
    let entry =
        keyring::Entry::new(SERVICE, provider_id).context("opening OS credential-store entry")?;
    entry
        .set_password(api_key)
        .context("saving API key to OS credential store")
}

pub(crate) fn load(provider_id: &str) -> Result<Option<String>> {
    let entry =
        keyring::Entry::new(SERVICE, provider_id).context("opening OS credential-store entry")?;
    match entry.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error).context("reading API key from OS credential store"),
    }
}

pub(crate) fn delete(provider_id: &str) -> Result<()> {
    let entry =
        keyring::Entry::new(SERVICE, provider_id).context("opening OS credential-store entry")?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error).context("deleting API key from OS credential store"),
    }
}
