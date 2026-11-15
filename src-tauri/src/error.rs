//! Crate-level error type for the Tauri command boundary.
//!
//! Every fallible `#[tauri::command]` returns `Result<T, Error>`. The manual
//! [`serde::Serialize`] impl serialises an
//! `Error` to its `Display` string, so the frontend keeps receiving a plain
//! string exactly as it did when commands returned `Result<T, String>`. That
//! preserves the error-string matching the UI relies on (the no-speech sentinel
//! `"Transcription produced no text"` and the `"Accessibility permission required"` hint).
//!
//! Variants:
//! - [`Error::Database`] and [`Error::Anyhow`] fold the existing source errors
//!   in via `#[from]`, so a command can `?`-propagate a [`DatabaseError`] or an
//!   `anyhow::Error` directly; `#[error(transparent)]` makes the serialised
//!   message identical to the source error's own `Display`.
//! - [`Error::Other`] is the context-carrying catch-all. Commands build a
//!   message with `format!`/`ok_or_else`/string literals and propagate it with
//!   `?`; the `From<String>`/`From<&str>` impls route it here, preserving the
//!   exact message the user used to see.

use crate::database::DatabaseError;

/// Error returned by Tauri commands.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A database-layer error, surfaced with its own `Display` message.
    #[error(transparent)]
    Database(#[from] DatabaseError),

    /// Any `anyhow::Error` from the transcription, enhancement, audio, or
    /// shortcut layers, surfaced with its `Display` message.
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),

    /// A context-carrying message built at the command boundary (the former
    /// `Result<_, String>` payload). Carries the user-facing text verbatim.
    #[error("{0}")]
    Other(String),

    /// A settings patch that does not merge into a valid config.
    #[error("merged config invalid: {0}")]
    InvalidPatch(String),
}

impl serde::Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Error::Other(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Error::Other(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The IPC contract: an `Error` serialises to a bare JSON string equal to
    /// its `Display`, so the frontend keeps receiving the same strings it did
    /// when commands returned `Result<T, String>`.
    #[test]
    fn other_serialises_to_its_message() {
        let e: Error = "Cannot copy empty text to clipboard".to_string().into();
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            "\"Cannot copy empty text to clipboard\""
        );
    }

    #[test]
    fn from_str_and_string_both_route_to_other_verbatim() {
        assert_eq!(Error::from("boom").to_string(), "boom");
        assert_eq!(Error::from("boom".to_string()).to_string(), "boom");
    }

    /// The no-speech sentinel the frontend matches on must serialise byte-for-byte.
    #[test]
    fn no_speech_sentinel_round_trips() {
        let e: Error = "Transcription produced no text".to_string().into();
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            "\"Transcription produced no text\""
        );
    }

    /// A folded-in `DatabaseError` source variant serialises to the source's
    /// own `Display` (transparent), unchanged from the previous string boundary.
    #[test]
    fn database_error_is_transparent() {
        let e: Error = crate::database::DatabaseError::Migration("schema v9".to_string()).into();
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            "\"Migration failed: schema v9\""
        );
    }

    #[test]
    fn anyhow_error_is_transparent() {
        let e: Error = anyhow::anyhow!("ane unavailable").into();
        assert_eq!(serde_json::to_string(&e).unwrap(), "\"ane unavailable\"");
    }
}
