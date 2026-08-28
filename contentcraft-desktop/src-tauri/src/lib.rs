use keyring_core::{set_default_store, Entry, Error as KeyringError};
use std::sync::LazyLock;

const SERVICE: &str = "com.contentcraft.app";

// One-time platform store registration — mirrors Lumia Career's approach exactly.
// On Windows the default CRED_PERSIST_ENTERPRISE fails on local (non-domain) accounts;
// we need CRED_PERSIST_LOCAL_MACHINE, set via the "persistence" modifier below.
static STORE_INIT: LazyLock<Result<(), String>> = LazyLock::new(|| {
    #[cfg(target_os = "windows")]
    let store = windows_native_keyring_store::Store::new().map_err(|e| e.to_string())?;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let store = apple_native_keyring_store::keychain::Store::new().map_err(|e| e.to_string())?;
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))]
    let store = zbus_secret_service_keyring_store::Store::new().map_err(|e| e.to_string())?;
    set_default_store(store);
    Ok(())
});

fn ensure_store() -> Result<(), String> {
    STORE_INIT.as_ref().map(|_| ()).map_err(|e| e.clone())
}

fn entry(provider: &str) -> Result<Entry, String> {
    ensure_store()?;
    #[cfg(target_os = "windows")]
    {
        use std::collections::HashMap;
        let mut modifiers = HashMap::new();
        // Force local persistence — required on Windows local accounts (non-domain).
        // Without this the credential "saves" but cannot be read back.
        modifiers.insert("persistence", "Local");
        return Entry::new_with_modifiers(SERVICE, provider, &modifiers)
            .map_err(|e| e.to_string());
    }
    #[cfg(not(target_os = "windows"))]
    Entry::new(SERVICE, provider).map_err(|e| e.to_string())
}

// ── Commands ──────────────────────────────────────────────────────────────

#[tauri::command]
fn set_api_key(provider: String, key: String) -> Result<(), String> {
    entry(&provider)?.set_password(&key).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_api_key(provider: String) -> Result<Option<String>, String> {
    match entry(&provider)?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn delete_api_key(provider: String) -> Result<(), String> {
    match entry(&provider)?.delete_credential() {
        Ok(_) => Ok(()),
        Err(KeyringError::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Returns the first Anthropic key found: keychain → ANTHROPIC_API_KEY env var.
#[tauri::command]
fn resolve_api_key() -> Option<String> {
    if let Ok(Some(key)) = get_api_key("anthropic".into()) {
        if !key.is_empty() { return Some(key); }
    }
    std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            set_api_key,
            get_api_key,
            delete_api_key,
            resolve_api_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
