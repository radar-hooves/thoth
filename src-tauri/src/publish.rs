//! Word-list publishing: write the dictionary and the canonical terms to one
//! shared JSON file on a WebDAV server (Nextcloud, ownCloud, any plain
//! WebDAV), so a second consumer — a phone keyboard, a gateway hook — can
//! read the same corrections with nothing but HTTP GET.
//!
//! Thoth is the single writer: publishing is one-way and the file is never
//! read back. The shared file carries both lists in one document
//! ([`SharedWordList`]); its format is documented for consumers in
//! `docs/word-list-publish.md`, and this module is its reference
//! implementation.
//!
//! Publishing is **off by default**. A user who never turns it on keeps the
//! purely local built-in lists: the loop wakes only when a list is saved, and
//! a wake with publishing disabled costs one config read, no network.

use crate::error::Error;
use base64::Engine as _;
use parking_lot::RwLock;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use crate::canonical::CanonicalTerm;
use crate::dictionary::DictionaryEntry;

/// Format version of the shared word-list document.
const SHARED_DOC_VERSION: u32 = 1;

/// How long to wait after a save before publishing, so a burst of edits — an
/// import, a rapid rename — becomes one PUT.
const DEBOUNCE: Duration = Duration::from_secs(3);

/// Per-request timeout for the WebDAV PUT.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// The shared document
// ---------------------------------------------------------------------------

/// The shared word-list document, version 1.
///
/// Rows are byte-identical to Thoth's own dictionary and canonical-term rows
/// (same field names via `rename_all = "camelCase"`), so `GET` the file and
/// read the arrays is all a second consumer ever needs to do.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedWordList {
    /// Format version, so a consumer can refuse a newer document rather than
    /// partially read it.
    pub version: u32,
    /// Dictionary find/replace rows, applied before canonical snapping.
    pub entries: Vec<DictionaryEntry>,
    /// Canonical terms with their aliases and snap policies.
    pub canonical_terms: Vec<CanonicalTerm>,
}

/// Snapshot both lists as they stand on this machine.
fn capture_local() -> SharedWordList {
    SharedWordList {
        version: SHARED_DOC_VERSION,
        entries: crate::dictionary::get_dictionary_entries().unwrap_or_default(),
        canonical_terms: crate::canonical::get_canonical_terms().unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// WebDAV client
// ---------------------------------------------------------------------------

/// A one-URL WebDAV client: plain HTTP PUT with basic auth, no locking.
struct WebDavClient {
    url: String,
    /// A pre-encoded `Basic ...` value. Never logged, never in status, never
    /// returned by any command.
    auth_header: Option<String>,
}

fn basic_auth_header(username: &str, password: &str) -> String {
    let raw = format!("{username}:{password}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw)
    )
}

/// Debug never carries the auth header — a debug print of the client must
/// not leak the credential.
impl std::fmt::Debug for WebDavClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebDavClient")
            .field("url", &self.url)
            .finish()
    }
}

impl WebDavClient {
    fn new(url: &str, username: &str, password: Option<&str>) -> Result<Self, String> {
        let parsed = url::Url::parse(url).map_err(|e| format!("Not a valid URL: {e}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(format!(
                "The URL must be http or https, not {}",
                parsed.scheme()
            ));
        }
        let auth_header = match (username.is_empty(), password) {
            (false, Some(pw)) if !pw.is_empty() => Some(basic_auth_header(username, pw)),
            // Without a username the PUT is anonymous and the server's own
            // 401 answers. A username with no stored password is reported
            // by `publish_once` before any request is made.
            _ => None,
        };
        Ok(Self {
            url: url.trim().to_string(),
            auth_header,
        })
    }

    /// A request builder with the Authorization header applied, if any. The
    /// URL is the user's own config value, not a credential.
    fn signed(
        &self,
        builder: reqwest_middleware::RequestBuilder,
    ) -> reqwest_middleware::RequestBuilder {
        match &self.auth_header {
            Some(h) => builder.header("Authorization", h),
            None => builder,
        }
    }

    /// PUT the document to the file.
    async fn put_doc(&self, doc: &SharedWordList) -> Result<(), String> {
        let body = serde_json::to_string_pretty(doc)
            .map_err(|e| format!("Failed to serialise the word list: {e}"))?;
        let response = tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.signed(crate::http_client().put(&self.url))
                .header("Content-Type", "application/json")
                .body(body)
                .send(),
        )
        .await
        .map_err(|_| format!("Timed out after {}s", REQUEST_TIMEOUT.as_secs()))?
        .map_err(|e| format!("The PUT failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("The server answered {} for PUT", response.status()));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Password store
// ---------------------------------------------------------------------------

/// Path to the dedicated password file (`~/.thoth/webdav_password`). Same
/// pattern as the control-API token store: a secret lives in its own
/// owner-only file, never inside `config.json`, which a settings reset or
/// schema change can clobber.
fn password_path() -> PathBuf {
    crate::config::get_config_dir().join("webdav_password")
}

/// Read the stored password, or `None` if absent or empty.
fn read_password() -> Option<String> {
    fs::read_to_string(password_path())
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|pw| !pw.is_empty())
}

/// Store or clear the password. Owner-only (`0o600`) because it is a secret.
fn write_password(password: Option<&str>) -> Result<(), String> {
    match password {
        Some(pw) => {
            let dir = crate::config::get_config_dir();
            fs::create_dir_all(&dir)
                .map_err(|e| format!("Failed to create the config directory: {e}"))?;
            let path = password_path();
            fs::write(&path, pw).map_err(|e| format!("Failed to write the password store: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(e) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
                    tracing::warn!("Failed to set password-store permissions to 0o600: {e}");
                }
            }
            Ok(())
        }
        None => match fs::remove_file(password_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Failed to clear the password store: {e}")),
        },
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// What the Settings pane shows about publishing. The password itself never
/// appears.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishStatus {
    pub has_password: bool,
    /// RFC 3339 timestamp of the last successful publish, if any
    pub last_publish_at: Option<String>,
    /// The last publish's failure, cleared by the next success
    pub last_error: Option<String>,
}

#[derive(Default, Clone)]
struct StatusInner {
    last_publish_at: Option<String>,
    last_error: Option<String>,
}

static STATUS: OnceLock<RwLock<StatusInner>> = OnceLock::new();

fn status() -> &'static RwLock<StatusInner> {
    STATUS.get_or_init(|| RwLock::new(StatusInner::default()))
}

fn record_success() {
    *status().write() = StatusInner {
        last_publish_at: Some(chrono::Utc::now().to_rfc3339()),
        last_error: None,
    };
}

fn record_error(message: &str) {
    status().write().last_error = Some(message.to_string());
}

// ---------------------------------------------------------------------------
// The publish
// ---------------------------------------------------------------------------

static PUBLISH: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn publish_mutex() -> &'static tokio::sync::Mutex<()> {
    PUBLISH.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// One publish, serialised so the debounced loop and the manual button never
/// interleave PUTs.
async fn publish_guarded(require_ready: bool) -> Result<(), String> {
    let _guard = publish_mutex().lock().await;
    publish_once(require_ready).await
}

/// Write the current lists to the WebDAV file.
///
/// `require_ready` turns "publishing is off (or has no URL)" into an error
/// for the manual command; the background loop passes `false` and treats it
/// as a quiet no-op — a wake costs one config read and nothing else.
async fn publish_once(require_ready: bool) -> Result<(), String> {
    let cfg = crate::config::get_config().map_err(|e| format!("Cannot read the config: {e}"))?;
    let publish_cfg = &cfg.publish;
    let url = publish_cfg.url.trim().to_string();
    if !publish_cfg.enabled || url.is_empty() {
        if require_ready {
            return Err("Word list publishing is off, or has no URL set".to_string());
        }
        return Ok(());
    }
    let password = read_password();
    if !publish_cfg.username.trim().is_empty() && password.is_none() {
        return Err(
            "A username is set but no password is stored — enter the password in \
             Settings → Integrations"
                .to_string(),
        );
    }

    let client = WebDavClient::new(&url, publish_cfg.username.trim(), password.as_deref())?;
    client.put_doc(&capture_local()).await?;
    record_success();
    Ok(())
}

// ---------------------------------------------------------------------------
// The debounced publish loop
// ---------------------------------------------------------------------------

static WAKE: OnceLock<tokio::sync::Notify> = OnceLock::new();

fn wake() -> &'static tokio::sync::Notify {
    WAKE.get_or_init(tokio::sync::Notify::new)
}

/// Tell publishing a local list changed. Called from `save_dictionary` and
/// `save_registry`, the two funnels every mutation path — UI, MCP, import —
/// already goes through. Harmless when publishing is off or no loop is
/// running.
pub fn notify_local_change() {
    wake().notify_one();
}

/// The publish loop: wake on a save, let the debounce window collect the
/// burst, then publish once. A save that lands during a publish stores a
/// permit, so it gets its own debounced publish afterwards.
pub fn spawn_publish_loop() {
    tauri::async_runtime::spawn(async {
        loop {
            wake().notified().await;
            tokio::time::sleep(DEBOUNCE).await;
            if let Err(e) = publish_guarded(false).await {
                record_error(&e);
                tracing::warn!("Word list publish failed: {e}");
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// The publishing state the Settings pane shows. Never the password itself.
#[tauri::command]
pub fn get_publish_status() -> PublishStatus {
    tauri_plugin_telemetry::traced_sync_value("get_publish_status", || {
        let inner = status().read().clone();
        PublishStatus {
            has_password: read_password().is_some(),
            last_publish_at: inner.last_publish_at,
            last_error: inner.last_error,
        }
    })
}

/// Store or clear (`None` or empty string) the WebDAV password. Write-only:
/// no command ever returns it.
#[tauri::command]
pub fn set_publish_password(password: Option<String>) -> Result<(), Error> {
    tauri_plugin_telemetry::traced_sync("set_publish_password", || {
        let password = password.filter(|pw| !pw.is_empty());
        write_password(password.as_deref()).map_err(Error::from)
    })
}

/// Publish now and report the outcome. The outcome (including any failure)
/// lands in the returned status rather than the command error, so the
/// Settings pane can show it.
#[tauri::command]
pub async fn publish_now() -> Result<PublishStatus, Error> {
    tauri_plugin_telemetry::traced("publish_now", async move {
        if let Err(e) = publish_guarded(true).await {
            record_error(&e);
        }
        Ok(get_publish_status())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::SnapPolicy;

    // -------------------------------------------------------------------------
    // Row builders
    // -------------------------------------------------------------------------

    fn entry(from: &str, to: &str) -> DictionaryEntry {
        DictionaryEntry {
            from: from.to_string(),
            to: to.to_string(),
            case_sensitive: false,
        }
    }

    fn doc(entries: Vec<DictionaryEntry>, terms: Vec<CanonicalTerm>) -> SharedWordList {
        SharedWordList {
            version: SHARED_DOC_VERSION,
            entries,
            canonical_terms: terms,
        }
    }

    // -------------------------------------------------------------------------
    // Document serialisation — the format second consumers read
    // -------------------------------------------------------------------------

    #[test]
    fn serialised_field_names_are_camelcase_for_second_consumers() {
        let text = serde_json::to_string(&doc(vec![entry("teh", "the")], vec![])).unwrap();
        assert!(text.contains("\"version\":1"));
        assert!(text.contains("\"caseSensitive\":false"));
        let with_term = serde_json::to_string(&doc(
            vec![],
            vec![CanonicalTerm {
                term: "X".to_string(),
                aliases: vec![],
                policy: SnapPolicy::Phonetic,
                max_words: 3,
                threshold: None,
            }],
        ))
        .unwrap();
        assert!(with_term.contains("\"maxWords\":3"));
        assert!(with_term.contains("\"policy\":\"phonetic\""));
    }

    // -------------------------------------------------------------------------
    // Client construction
    // -------------------------------------------------------------------------

    #[test]
    fn client_accepts_http_and_https_only() {
        assert!(
            WebDavClient::new(
                "https://cloud.example/remote.php/dav/files/me/words.json",
                "me",
                Some("pw")
            )
            .is_ok()
        );
        assert!(WebDavClient::new("http://nas.local/words.json", "me", Some("pw")).is_ok());
        let err =
            WebDavClient::new("ftp://cloud.example/words.json", "me", Some("pw")).unwrap_err();
        assert!(err.contains("http or https"));
    }

    #[test]
    fn basic_auth_header_is_the_standard_encoding() {
        assert_eq!(basic_auth_header("user", "pass"), "Basic dXNlcjpwYXNz");
    }

    #[test]
    fn client_without_username_sends_no_auth_header() {
        let client = WebDavClient::new("https://cloud.example/words.json", "", Some("pw")).unwrap();
        assert!(client.auth_header.is_none());
    }

    #[test]
    fn debug_never_carries_the_auth_header() {
        let client =
            WebDavClient::new("https://cloud.example/words.json", "user", Some("pass")).unwrap();
        let printed = format!("{client:?}");
        assert!(printed.contains("cloud.example"));
        assert!(
            !printed.contains("dXNlcjpwYXNz"),
            "debug printed the credential"
        );
    }

    // -------------------------------------------------------------------------
    // WebDAV PUT over mockito
    // -------------------------------------------------------------------------

    fn file_url(server: &mockito::ServerGuard) -> String {
        format!("{}/words.json", server.url())
    }

    #[tokio::test]
    async fn put_writes_the_whole_document() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("PUT", "/words.json")
            .match_header("content-type", "application/json")
            .match_header("authorization", "Basic dXNlcjpwYXNz")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"version":1,"entries":[{"from":"teh","to":"the","caseSensitive":false}]}"#
                    .to_string(),
            ))
            .with_status(201)
            .create_async()
            .await;

        let client = WebDavClient::new(&file_url(&server), "user", Some("pass")).unwrap();
        client
            .put_doc(&doc(vec![entry("teh", "the")], vec![]))
            .await
            .unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn put_error_status_surfaces_the_status() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("PUT", "/words.json")
            .with_status(403)
            .create_async()
            .await;

        let client = WebDavClient::new(&file_url(&server), "", None).unwrap();
        let err = client.put_doc(&doc(vec![], vec![])).await.unwrap_err();
        assert!(err.contains("403"), "unexpected error: {err}");
    }

    // The password store is deliberately untested here: it writes the real
    // `~/.thoth`, which `get_config_dir()` points at the operator's home with
    // no test override, and touching a real stored credential from a test is
    // worse than not testing a ten-line file write.
}
