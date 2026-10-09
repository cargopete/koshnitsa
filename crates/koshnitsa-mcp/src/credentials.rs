use anyhow::{Context, Result};
use koshnitsa_client::Cookie;

const SERVICE: &str = "koshnitsa";
const ACCOUNT: &str = "ebag-cookie";
pub const ENV_OVERRIDE: &str = "EBAG_COOKIE";

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT).context("opening the OS keychain")
}

/// The environment wins over the keychain so CI and throwaway shells need no keychain at all.
pub fn load() -> Result<Option<Cookie>> {
    if let Ok(raw) = std::env::var(ENV_OVERRIDE) {
        return Ok(Some(Cookie::parse(&raw)?));
    }
    match entry()?.get_password() {
        Ok(raw) => Ok(Some(Cookie::parse(&raw)?)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("reading the eBag cookie from the keychain"),
    }
}

pub fn store(cookie: &Cookie) -> Result<()> {
    entry()?
        .set_password(cookie.header())
        .context("writing the eBag cookie to the keychain")
}

pub fn delete() -> Result<bool> {
    match entry()?.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(e).context("removing the eBag cookie from the keychain"),
    }
}
