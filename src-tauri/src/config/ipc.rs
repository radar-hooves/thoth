//! The webview's door onto the config: the same four operations as the typed
//! functions in the parent module, returning camelCase JSON. Rust callers use
//! the typed functions; only the webview goes through these.

use super::{Config, camelise_keys};
use crate::error::Error;

fn camelised(config: &Config) -> Result<serde_json::Value, Error> {
    serde_json::to_value(config)
        .map(camelise_keys)
        .map_err(|e| Error::Other(format!("Failed to serialise config: {e}")))
}

#[tauri::command]
pub fn get_config() -> Result<serde_json::Value, Error> {
    camelised(&super::get_config()?)
}

#[tauri::command]
pub fn get_default_config() -> Result<serde_json::Value, Error> {
    camelised(&super::get_default_config())
}

#[tauri::command]
pub fn patch_config(patch: serde_json::Value) -> Result<serde_json::Value, Error> {
    camelised(&super::patch_config(patch)?)
}

#[tauri::command]
pub async fn reset_config() -> Result<serde_json::Value, Error> {
    camelised(&super::reset_config().await?)
}
