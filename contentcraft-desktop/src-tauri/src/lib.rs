use keyring::Entry;

const SERVICE: &str = "contentcraft";

#[tauri::command]
fn set_api_key(provider: String, key: String) -> Result<(), String> {
    Entry::new(SERVICE, &provider)
        .map_err(|e| e.to_string())?
        .set_password(&key)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_api_key(provider: String) -> Result<Option<String>, String> {
    let entry = Entry::new(SERVICE, &provider).map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn delete_api_key(provider: String) -> Result<(), String> {
    Entry::new(SERVICE, &provider)
        .map_err(|e| e.to_string())?
        .delete_credential()
        .map_err(|e| e.to_string())
}

/// Returns the first Anthropic key found: keychain → ANTHROPIC_API_KEY env var.
#[tauri::command]
fn resolve_api_key() -> Option<String> {
    // 1. Check keychain (previously saved via older app version)
    if let Ok(entry) = Entry::new(SERVICE, "anthropic") {
        if let Ok(key) = entry.get_password() {
            if !key.is_empty() {
                return Some(key);
            }
        }
    }
    // 2. Fall back to environment variable (works with Claude Code / dev env)
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
