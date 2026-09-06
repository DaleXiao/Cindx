//! macOS Keychain backing for the provider API key.
//!
//! The private config file keeps only a credential *reference*: on save the key
//! is written to the login keychain through the Security.framework API and the
//! on-disk `api_key` field is cleared; on load an empty field is filled from the
//! keychain. Saving recreates the item (delete, then add) so the saving build
//! is the item's creator and implicitly trusted — updating in place would
//! preserve a legacy ACL that keeps prompting (see `store_provider_api_key`).
//! Legacy conf files that still carry a plaintext key are migrated on
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

/// How long any keychain read may take before the app gives up on it. The read is
/// a `SecItemCopyMatching`, which waits for a SecurityAgent authorization prompt
/// when the calling binary's signature is not in the item's ACL — and that prompt
/// cannot be rendered while the session is locked or the display is asleep, so an
/// unbounded read hangs startup with no window and no log line. Bounding it turns
/// that hang into a degraded start the user can see and recover from.
#[cfg(target_os = "macos")]
pub(crate) const KEYCHAIN_READ_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(1500);

/// `errSecItemNotFound`: clearing an absent entry counts as success, matching
/// the historical CLI behavior (exit code 44).
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
pub(crate) fn store_provider_api_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return clear_provider_api_key();
    }
    // Recreate rather than update: the framework's set is find-then-update,
    // and an update preserves the item's existing ACL. A legacy item created
    // by an older ad-hoc-signed build keeps prompting for authorization on
    // every launch, because that build's cdhash-based requirement can never
    // match a newer binary. Deleting first makes the current binary the
    // item's creator, and the creating application is implicitly trusted;
    // with the stable local signing identity that trust survives rebuilds,
    // so saving the key once ends the prompts for good. The delete may raise
    // one authorization prompt for a legacy item — the user is present, since
    // they are saving the key interactively.
    match delete_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
        Ok(()) => {}
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => {}
        Err(error) => {
            return Err(format!(
                "keychain could not replace the provider credential: {error}"
            ))
        }
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

/// The same read, bounded. Returns `None` when the read did not finish within
/// [`KEYCHAIN_READ_TIMEOUT`] — the signature of a keychain waiting on a prompt that
/// cannot be shown — in which case the caller degrades instead of waiting, and the
/// reason is written to the startup log. A read that finishes late is dropped: its
/// thread exits against a closed channel.
#[cfg(target_os = "macos")]
pub(crate) fn read_provider_api_key_bounded(timeout: std::time::Duration) -> Option<String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(read_provider_api_key());
    });
    match receiver.recv_timeout(timeout) {
        Ok(key) => Some(key),
        Err(_) => {
            crate::persistence_runtime::append_startup_log(
                "provider api key read did not complete within the startup bound; the keychain is probably waiting on an authorization prompt that cannot be shown (locked or asleep session). Starting without the key: unlock the session and the next run re-reads it, or re-enter the key in Settings.",
            );
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn read_provider_api_key_bounded(_timeout: std::time::Duration) -> Option<String> {
    Some(String::new())
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

/// Re-read a missing API key from the keychain, bounded, and cache a success back
/// into the managed config so the recovery happens once. A startup read degrades to
/// "no key" when the session was locked or the display asleep; this is what makes
/// unlocking the session heal the next run without the user re-entering anything.
/// When the key is present it does nothing, so the common path costs no read.
pub(crate) fn refresh_missing_api_key(
    state: &crate::app_state::AppState,
    config: &mut ProviderConfig,
) {
    if !config.api_key.is_empty() {
        return;
    }
    let Some(key) = read_provider_api_key_bounded(KEYCHAIN_READ_TIMEOUT) else {
        return;
    };
    if key.is_empty() {
        return;
    }
    config.api_key = key.clone();
    if let Ok(mut stored) = state.provider_config.lock() {
        if stored.api_key.is_empty() {
            stored.api_key = key;
        }
    }
}

/// The conf file carries only a credential reference: an empty api_key is
/// filled from the login keychain. Legacy plaintext keys stay in memory
/// until the next save migrates them. The read is bounded: a keychain that
/// cannot show its authorization prompt (locked or asleep session) degrades
/// to an empty key rather than hanging startup.
pub(crate) fn fill_api_key_from_keychain(config: &mut ProviderConfig) {
    if config.api_key.is_empty() {
        config.api_key = read_provider_api_key_bounded(KEYCHAIN_READ_TIMEOUT).unwrap_or_default();
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// The bound is the contract. Whatever the keychain is doing — item present,
    /// item absent, or waiting on an authorization prompt that cannot be shown —
    /// the call returns inside the bound plus scheduling slack, so startup can
    /// never hang on it again.
    #[test]
    fn a_bounded_keychain_read_returns_inside_its_bound() {
        let started = std::time::Instant::now();
        let _key = read_provider_api_key_bounded(KEYCHAIN_READ_TIMEOUT);
        let elapsed = started.elapsed();
        assert!(
            elapsed < KEYCHAIN_READ_TIMEOUT + std::time::Duration::from_secs(3),
            "the bounded keychain read took {elapsed:?}"
        );
    }
}
