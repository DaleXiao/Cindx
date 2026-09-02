//! macOS Keychain backing for the provider API key.
//!
//! The private config file keeps only a credential *reference*: on save the key
//! is written to the login keychain through the Security.framework API and the
//! on-disk `api_key` field is cleared; on load an empty field is filled from the
//! keychain. Legacy conf files that still carry a plaintext key are migrated on
//! the next save. The secret is handed to the framework in-process and never
//! appears in argv, the environment, logs, or persisted events. If the keychain
//! is unavailable the save falls back to the historical private (0600) conf file
//! so the app never loses the ability to run, and the fallback is recorded in
//! the startup log.

#[cfg(target_os = "macos")]
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

use crate::configuration_models::ProviderConfig;

const KEYCHAIN_SERVICE: &str = "Cindx provider";
const KEYCHAIN_ACCOUNT: &str = "api-key";

/// `errSecItemNotFound`: clearing an absent entry counts as success, matching
/// the historical CLI behavior (exit code 44).
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
pub(crate) fn store_provider_api_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return clear_provider_api_key();
    }
    set_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, key.as_bytes())
        .map_err(|error| format!("keychain rejected the provider credential: {error}"))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn store_provider_api_key(_key: &str) -> Result<(), String> {
    Err("the provider keychain is only available on macOS".to_string())
}

#[cfg(target_os = "macos")]
pub(crate) fn read_provider_api_key() -> String {
    match get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).trim().to_string(),
        Err(_) => String::new(),
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn read_provider_api_key() -> String {
    String::new()
}

#[cfg(target_os = "macos")]
pub(crate) fn clear_provider_api_key() -> Result<(), String> {
    match delete_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
        Ok(()) => Ok(()),
        // Absent entries are fine: clearing something that is not there succeeds.
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        Err(error) => Err(format!(
            "keychain could not clear the provider credential: {error}"
        )),
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn clear_provider_api_key() -> Result<(), String> {
    Ok(())
}

/// Clone of the config safe to serialize: the API key is moved into the login
/// keychain and cleared from the on-disk copy. When the keychain is
/// unavailable the historical private (0600) conf remains as a logged
/// fallback so the app never loses the credential.
pub(crate) fn config_for_disk(config: &ProviderConfig) -> ProviderConfig {
    let mut disk = config.clone();
    if !disk.api_key.is_empty() {
        match store_provider_api_key(&disk.api_key) {
            Ok(()) => disk.api_key.clear(),
            Err(error) => crate::persistence_runtime::append_startup_log(&format!(
                "provider api key could not be stored in the keychain ({error}); falling back to the private config file"
            )),
        }
    }
    disk
}

/// The conf file carries only a credential reference: an empty api_key is
/// filled from the login keychain. Legacy plaintext keys stay in memory
/// until the next save migrates them.
pub(crate) fn fill_api_key_from_keychain(config: &mut ProviderConfig) {
    if config.api_key.is_empty() {
        config.api_key = read_provider_api_key();
    }
}

/// Whether a provider base URL may carry the API key. TLS endpoints always
/// may; plaintext `http://` is restricted to loopback so a long-lived
/// credential can never be sent in the clear to an arbitrary host.
pub(crate) fn validate_provider_base_url_for_credentials(base_url: &str) -> Result<(), String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let lower = trimmed.to_lowercase();
    if let Some(rest) = lower.strip_prefix("http://") {
        let host = if rest.starts_with('[') {
            rest.find(']')
                .map(|end| rest[..end].trim_start_matches('[').to_string())
                .unwrap_or_default()
        } else {
            rest.split(['/', ':', '?'])
                .next()
                .unwrap_or_default()
                .to_string()
        };
        if matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1") {
            return Ok(());
        }
        return Err(
            "plaintext http provider endpoints may only target loopback; use https for remote providers"
                .to_string(),
        );
    }
    if lower.starts_with("https://") {
        return Ok(());
    }
    Err(format!("unsupported provider scheme: {trimmed}"))
}
