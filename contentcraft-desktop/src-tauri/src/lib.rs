use keyring_core::{set_default_store, Entry, Error as KeyringError};
use serde::Serialize;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

const SERVICE: &str = "com.contentcraft.app";

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
        modifiers.insert("persistence", "Local");
        return Entry::new_with_modifiers(SERVICE, provider, &modifiers)
            .map_err(|e| e.to_string());
    }
    #[cfg(not(target_os = "windows"))]
    Entry::new(SERVICE, provider).map_err(|e| e.to_string())
}

// ── Keychain commands ──────────────────────────────────────────────────────

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

fn read_from_service(service: &str, provider: &str) -> Option<String> {
    ensure_store().ok()?;
    #[cfg(target_os = "windows")]
    {
        use std::collections::HashMap;
        let mut modifiers = HashMap::new();
        modifiers.insert("persistence", "Local");
        let e = Entry::new_with_modifiers(service, provider, &modifiers).ok()?;
        let k = e.get_password().ok()?;
        if k.is_empty() { return None; }
        return Some(k);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let e = Entry::new(service, provider).ok()?;
        let k = e.get_password().ok()?;
        if k.is_empty() { return None; }
        Some(k)
    }
}

// ── Claude Code CLI ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ClaudeCliStatus {
    pub found: bool,
    pub version: Option<String>,
}

// On Unix/Mac, locate `claude` via `which` so we can invoke it directly.
#[cfg(not(target_os = "windows"))]
fn find_claude_unix() -> Option<String> {
    let out = Command::new("which").arg("claude").output().ok()?;
    if out.status.success() {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !p.is_empty() { return Some(p); }
    }
    None
}

// On Windows, test reachability by running `cmd /C claude --version`.
// cmd.exe re-reads the user-level PATH from the registry on launch, so it finds
// npm global shims (claude.cmd) even when the parent GUI process didn't inherit them.
#[cfg(target_os = "windows")]
fn probe_claude_windows() -> Option<String> {
    let out = Command::new("cmd")
        .args(["/C", "claude", "--version"])
        .output().ok()?;
    if out.status.success() {
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Some(if v.is_empty() { "unknown".to_string() } else { v })
    } else {
        None
    }
}

#[tauri::command]
fn detect_claude_cli() -> ClaudeCliStatus {
    #[cfg(target_os = "windows")]
    {
        match probe_claude_windows() {
            Some(v) => ClaudeCliStatus { found: true, version: Some(v) },
            None    => ClaudeCliStatus { found: false, version: None },
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        match find_claude_unix() {
            Some(path) => {
                let version = Command::new(&path).arg("--version").output().ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
                ClaudeCliStatus { found: true, version }
            }
            None => ClaudeCliStatus { found: false, version: None },
        }
    }
}

/// Run a prompt through `claude -p` headless mode.
///
/// - Prompt via stdin — NEVER as CLI arg (Windows cmd.exe truncates at ~8 191 chars)
/// - On Windows: `cmd /C claude -p` — cmd.exe resolves the .cmd shim via its own PATH
/// - stdin written in a thread to avoid pipe-buffer deadlock on large prompts
/// - stderr returned verbatim so "Not logged in" messages reach the user
#[tauri::command]
fn claude_cli_generate(prompt: String) -> Result<String, String> {
    #[cfg(not(target_os = "windows"))]
    let path = find_claude_unix()
        .ok_or_else(|| "claude CLI no encontrado. Instala Claude Code y ejecuta `claude` una vez para iniciar sesión.".to_string())?;

    let mut child = {
        #[cfg(target_os = "windows")]
        { Command::new("cmd").args(["/C", "claude", "-p"]) }
        #[cfg(not(target_os = "windows"))]
        { let mut c = Command::new(&path); c.arg("-p"); c }
    }
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|e| format!("No se pudo iniciar claude: {e}"))?;

    // Write prompt in a thread — avoids deadlock when prompt > OS pipe buffer (~64 KB)
    let prompt_bytes = prompt.into_bytes();
    let mut stdin = child.stdin.take().ok_or("stdin no disponible")?;
    let writer = std::thread::spawn(move || stdin.write_all(&prompt_bytes));

    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join(); // writer may have already exited if process ended early

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        // Return stderr verbatim — may say "Not logged in — Please run /login"
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Err(if !stderr.is_empty() { stderr }
            else if !stdout.is_empty() { stdout }
            else { format!("claude falló (código {})", output.status.code().unwrap_or(-1)) })
    }
}

// ── Resolve API key ────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn read_registry_env(name: &str) -> Option<String> {
    let out = Command::new("reg")
        .args(["query", r"HKCU\Environment", "/v", name])
        .output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
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
        .or_else(|_| std::env::var("HOME")).ok()?;
    let contents = std::fs::read_to_string(
        std::path::Path::new(&home).join(".claude").join("settings.json")
    ).ok()?;
    let v: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let k = v.get("apiKey")?.as_str()?.to_string();
    if k.is_empty() { None } else { Some(k) }
}

#[tauri::command]
fn resolve_api_key() -> Option<String> {
    // 1. ContentCraft keychain
    if let Ok(Some(k)) = get_api_key("anthropic".into()) {
        if !k.is_empty() { return Some(k); }
    }
    // 2. Lumia Career keychain (user may already have key saved there)
    if let Some(k) = read_from_service("com.lumiacloud.lumiacareer", "anthropic") {
        return Some(k);
    }
    // 3. Process env var
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        if !k.is_empty() { return Some(k); }
    }
    // 4. Windows registry user env (GUI apps don't inherit shell env)
    #[cfg(target_os = "windows")]
    if let Some(k) = read_registry_env("ANTHROPIC_API_KEY") {
        return Some(k);
    }
    // 5. Claude Code settings.json
    if let Some(k) = read_claude_settings_key() {
        return Some(k);
    }
    None
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
