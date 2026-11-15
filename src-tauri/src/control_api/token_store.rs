//! Stable, install-independent storage for the Control-API / MCP bearer token.
//!
//! The token lives in its own file (`~/.thoth/control_api_token`, owner-only)
//! rather than inside `config.json`. `config.json` is mutable and reset-prone —
//! a settings reset, a schema change, or a reinstall can clobber it — and any
//! such event previously regenerated the token, silently breaking every MCP
//! client pointed at the old value. Pinning the token to a dedicated file means
//! it survives config resets and reinstalls, so the token is generated exactly
//! once per machine and never rotates unless the user explicitly asks.

use std::fs;
use std::io;
use std::path::PathBuf;

use super::generate_token;

/// Path to the dedicated token file (`~/.thoth/control_api_token`).
fn token_path() -> PathBuf {
    crate::config::data_dir().join("control_api_token")
}

/// Read the persisted token, or `None` if the store is absent or empty.
pub fn read_token() -> Option<String> {
    let raw = fs::read_to_string(token_path()).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Persist the token to the store, creating `~/.thoth` if needed and
/// restricting the file to owner-only (`0o600`) because it is a secret.
fn write_token(token: &str) -> io::Result<()> {
    desktop_shell::secrets::write_private(&token_path(), token.as_bytes())
}

/// One-time migration source: the token a previous version stored inside
/// `config.json`. Read straight from the raw JSON (camelCase `apiToken`) so the
/// migration still works after the typed field is removed from the config
/// struct. Returns `None` if config.json is absent or has no token.
fn legacy_config_token() -> Option<String> {
    let contents = fs::read_to_string(crate::config::get_config_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let token = value.get("integrations")?.get("apiToken")?.as_str()?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Choose the token to use, in precedence order: an already-stored token wins;
/// otherwise migrate the token a previous version left in config.json (so an
/// existing user keeps the token their MCP clients already trust); otherwise
/// mint a fresh one. Pure so the precedence is unit-testable.
fn resolve(stored: Option<String>, legacy: Option<String>) -> String {
    stored.or(legacy).unwrap_or_else(generate_token)
}

/// Return the stable bearer token, creating it once if the store is empty.
///
/// On first run after upgrading, the user's existing `config.json` token is
/// migrated into the store, so the token does not change on upgrade. Every
/// subsequent launch returns the identical value from the store file.
pub fn get_or_create_token() -> String {
    if let Some(token) = read_token() {
        return token;
    }
    let token = resolve(None, legacy_config_token());
    if let Err(e) = write_token(&token) {
        tracing::error!("Failed to persist control-API token store: {}", e);
    }
    token
}

/// Generate a fresh token, persist it to the store, and return it. This is the
/// only path that intentionally changes the token (the Integrations pane's
/// "rotate" action).
pub fn rotate() -> String {
    let token = generate_token();
    if let Err(e) = write_token(&token) {
        tracing::error!("Failed to persist rotated control-API token: {}", e);
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_token_wins_over_legacy_and_generation() {
        let t = resolve(
            Some("sk-thoth-stored".to_string()),
            Some("sk-thoth-legacy".to_string()),
        );
        assert_eq!(t, "sk-thoth-stored");
    }

    #[test]
    fn legacy_token_is_migrated_when_store_empty() {
        // The core guarantee: an upgrading user keeps their existing token.
        let t = resolve(None, Some("sk-thoth-existing".to_string()));
        assert_eq!(t, "sk-thoth-existing");
    }

    #[test]
    fn fresh_token_minted_when_nothing_to_migrate() {
        let t = resolve(None, None);
        assert!(
            t.starts_with("sk-thoth-"),
            "should mint a canonical token: {t}"
        );
    }
}
