//! Passwords of data sources and the AI service's API key, in the system keychain (macOS Keychain, Windows
//! Credential Manager, or the Secret Service on Linux). Nothing secret is written to the app's database or files.
//!
//! A secret is read from the keychain once per run of the app and then kept in memory, so the keychain
//! does not ask for permission on every query or every AI call. Saving or removing a secret updates that copy.

use keyring::Entry;
use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "sqlpool";

/// The secrets read so far this run, by keychain account name. `None` is cached too: "no such secret".
static CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

fn cached(account: &str) -> Option<Option<String>> {
    CACHE.lock().ok()?.as_ref()?.get(account).cloned()
}

fn remember(account: &str, value: Option<String>) {
    if let Ok(mut guard) = CACHE.lock() {
        guard.get_or_insert_with(HashMap::new).insert(account.to_string(), value);
    }
}

fn entry_for(account: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, account).map_err(|e| format!("keychain: {e}"))
}

fn read(account: &str) -> Result<Option<String>, String> {
    if let Some(hit) = cached(account) {
        return Ok(hit);
    }
    let value = match entry_for(account)?.get_password() {
        Ok(p) => Some(p),
        Err(keyring::Error::NoEntry) => None,
        Err(e) => return Err(format!("keychain: {e}")),
    };
    remember(account, value.clone());
    Ok(value)
}

fn write(account: &str, value: &str) -> Result<(), String> {
    entry_for(account)?
        .set_password(value)
        .map_err(|e| format!("keychain: {e}"))?;
    remember(account, Some(value.to_string()));
    Ok(())
}

fn remove(account: &str) -> Result<(), String> {
    match entry_for(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(format!("keychain: {e}")),
    }
    remember(account, None);
    Ok(())
}

fn datasource_account(datasource_id: i64) -> String {
    format!("datasource:{datasource_id}")
}

fn named_account(name: &str) -> String {
    format!("named:{name}")
}

pub fn set(datasource_id: i64, password: &str) -> Result<(), String> {
    write(&datasource_account(datasource_id), password)
}

/// The saved password, or `None` when there is none.
pub fn get(datasource_id: i64) -> Result<Option<String>, String> {
    read(&datasource_account(datasource_id))
}

pub fn delete(datasource_id: i64) -> Result<(), String> {
    remove(&datasource_account(datasource_id))
}

/// A named secret (not tied to a data source), such as the AI service's API key.
pub fn set_named(name: &str, value: &str) -> Result<(), String> {
    write(&named_account(name), value)
}

pub fn get_named(name: &str) -> Result<Option<String>, String> {
    read(&named_account(name))
}

pub fn delete_named(name: &str) -> Result<(), String> {
    remove(&named_account(name))
}
