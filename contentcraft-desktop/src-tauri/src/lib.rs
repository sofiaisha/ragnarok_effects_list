use keyring_core::{set_default_store, Entry, Error as KeyringError};
use serde::Serialize;
use std::process::Command;
use std::sync::LazyLock;

const SERVICE: &str = "com.contentcraft.app";

// One-time platform store registration.
// On Windows the default CRED_PERSIST_ENTERPRISE fails on local (non-domain) accounts;
// we need CRED_PERSIST_LOCAL_MACHINE set via the "persistence" modifier.
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
        modifiers.insert("persistence", "Local");
        return Entry::new_with_modifiers(SERVICE, provider, &modifiers)
            .map_err(|e| e.to_string());
    }
    #[cfg(not(target_os = "windows"))]
    Entry::new(SERVICE, provider).map_err(|e| e.to_string())
}

// ── Keychain commands ─────────────────────────────────────────────────────

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

// ── Claude Code detection (same approach as Lumia Career) ─────────────────

#[derive(Serialize)]
pub struct ClaudeCliStatus {
    pub found: bool,
    pub version: Option<String>,
}

/// Detects whether the `claude` CLI is available.
/// On Windows, npm shims are .cmd files — try cmd /C, then PowerShell, then known paths.
#[tauri::command]
fn detect_claude_cli() -> ClaudeCliStatus {
    #[cfg(target_os = "windows")]
    {
        // Try cmd /C first (works when npm bin dir is in system/user PATH)
        let attempts: &[&[&str]] = &[
            &["cmd", "/C", "claude", "--version"],
            &["powershell", "-Command", "claude --version"],
        ];
        for args in attempts {
            if let Ok(o) = Command::new(args[0]).args(&args[1..]).output() {
                if o.status.success() {
                    return ClaudeCliStatus {
                        found: true,
                        version: Some(String::from_utf8_lossy(&o.stdout).trim().to_string()),
                    };
                }
            }
        }
        // Try known npm global locations directly
        let appdata = std::env::var("APPDATA").unwrap_or_default();
        let candidates = [
            format!(r"{appdata}\npm\claude.cmd"),
            format!(r"{}\AppData\Roaming\npm\claude.cmd",
                    std::env::var("USERPROFILE").unwrap_or_default()),
        ];
        for path in &candidates {
            if std::path::Path::new(path).exists() {
                return ClaudeCliStatus { found: true, version: None };
            }
        }
        ClaudeCliStatus { found: false, version: None }
    }
    #[cfg(not(target_os = "windows"))]
    match Command::new("claude").arg("--version").output() {
        Ok(o) if o.status.success() => ClaudeCliStatus {
            found: true,
            version: Some(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        },
        _ => ClaudeCliStatus { found: false, version: None },
    }
}

// ── Resolve API key from all sources ─────────────────────────────────────
//
// Priority: (1) ContentCraft keychain, (2) ANTHROPIC_API_KEY process env,
// (3) Windows registry user env vars (GUI apps don't inherit shell env vars),
// (4) Claude Code credentials file (~/.claude/.credentials.json — OAuth token,
//     not usable as Anthropic API key, so skipped),
// (5) Claude Code settings.json apiKey field.

#[cfg(target_os = "windows")]
fn read_registry_env(name: &str) -> Option<String> {
    // HKCU\Environment holds per-user env vars set via System Properties.
    // GUI apps launched outside a shell don't inherit these from the process env.
    use std::process::Command;
    let output = Command::new("reg")
        .args(["query", r"HKCU\Environment", "/v", name])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    // reg query output: "    NAME    REG_SZ    VALUE"
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(3, "REG_SZ").collect();
        if parts.len() == 2 {
            let val = parts[1].trim().to_string();
            if !val.is_empty() { return Some(val); }
        }
    }
    None
}

fn read_claude_settings_key() -> Option<String> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    let path = std::path::Path::new(&home).join(".claude").join("settings.json");
    let contents = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let key = v.get("apiKey")?.as_str()?.to_string();
    if key.is_empty() { None } else { Some(key) }
}

/// Run a prompt through `claude -p` (headless mode). Uses Claude Code's own auth.
#[tauri::command]
fn claude_cli_generate(prompt: String) -> Result<String, String> {
    use std::io::Write;

    let spawn_result = if cfg!(windows) {
        // Try cmd /C first, then powershell
        Command::new("cmd")
            .args(["/C", "claude", "-p"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .or_else(|_| {
                Command::new("powershell")
                    .args(["-Command", "claude -p"])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
            })
    } else {
        Command::new("claude")
            .arg("-p")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    };

    let mut child = spawn_result.map_err(|e| format!("No se pudo iniciar claude: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).map_err(|e| e.to_string())?;
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&output.stderr).to_string();
        Err(if err.is_empty() { "claude CLI sin respuesta".into() } else { err })
    }
}

#[tauri::command]
fn resolve_api_key() -> Option<String> {
    // 1. ContentCraft's own keychain entry
    if let Ok(Some(k)) = get_api_key("anthropic".into()) {
        if !k.is_empty() { return Some(k); }
    }
    // 2. Lumia Career's keychain entry — user may have saved key there already
    if let Some(k) = read_from_service("com.lumiacloud.lumiacareer", "anthropic") {
        return Some(k);
    }
    // 3. Process environment variable
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        if !k.is_empty() { return Some(k); }
    }
    // 4. Windows registry user environment (not inherited by GUI apps)
    #[cfg(target_os = "windows")]
    if let Some(k) = read_registry_env("ANTHROPIC_API_KEY") {
        return Some(k);
    }
    // 5. Claude Code settings.json apiKey field
    if let Some(k) = read_claude_settings_key() {
        return Some(k);
    }
    None
}

fn read_from_service(service: &str, provider: &str) -> Option<String> {
    ensure_store().ok()?;
    #[cfg(target_os = "windows")]
    {
        use std::collections::HashMap;
        let mut modifiers = HashMap::new();
        modifiers.insert("persistence", "Local");
        let entry = Entry::new_with_modifiers(service, provider, &modifiers).ok()?;
        let k = entry.get_password().ok()?;
        if k.is_empty() { return None; }
        return Some(k);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let entry = Entry::new(service, provider).ok()?;
        let k = entry.get_password().ok()?;
        if k.is_empty() { return None; }
        Some(k)
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            set_api_key,
            get_api_key,
            delete_api_key,
            detect_claude_cli,
            claude_cli_generate,
            resolve_api_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
