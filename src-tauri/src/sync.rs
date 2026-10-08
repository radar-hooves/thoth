//! Word-list sync: keep the dictionary and canonical terms in step with a
//! shared JSON file on a WebDAV server (Nextcloud, ownCloud, any plain WebDAV).
//!
//! The shared file carries both lists in one document ([`SharedWordList`]), so
//! a second consumer — a phone keyboard, a gateway hook — can read and write
//! the same corrections with nothing but HTTP GET and PUT. The format is
//! documented for those consumers in `docs/word-list-sync.md`; this module is
//! its reference implementation.
//!
//! Sync is **off by default**. A user who never turns it on keeps the purely
//! local built-in lists: the loop is self-gating and a tick with sync disabled
//! costs one config read, no network.
//!
//! Timing: a local list change (any mutation path — UI, MCP, import — they all
//! funnel through `save_dictionary`/`save_registry`, which call
//! [`notify_local_change`]) is pushed within one tick (5 s); otherwise the
//! remote file is polled every 60 s. Conflicts — the same entry changed on
//! both sides since the last sync — resolve to this Thoth's version; every
//! other change propagates from whichever side made it.

use crate::error::Error;
use base64::Engine as _;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::canonical::CanonicalTerm;
use crate::dictionary::DictionaryEntry;

/// Format version of the shared word-list document.
const SHARED_DOC_VERSION: u32 = 1;

/// Loop tick: how soon a local change is pushed, and the poll granularity.
const TICK: Duration = Duration::from_secs(5);

/// Poll the remote file every this many ticks (60 s at [`TICK`]).
const PULL_EVERY_TICKS: u32 = 12;

/// Per-request timeout for the WebDAV GET/PUT.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// The shared document
// ---------------------------------------------------------------------------

/// The shared word-list document, version 1.
///
/// Rows are byte-identical to Thoth's own dictionary and canonical-term rows
/// (same field names via `rename_all = "camelCase"`), so `GET` the file, edit
/// an array, `PUT` it back is a valid workflow for a second consumer or a
/// hand edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedWordList {
    /// Format version. Thoth refuses a file with a higher version rather than
    /// silently dropping rows it cannot understand.
    #[serde(default = "default_doc_version")]
    pub version: u32,
    /// Dictionary find/replace rows, applied before canonical snapping.
    #[serde(default)]
    pub entries: Vec<DictionaryEntry>,
    /// Canonical terms with their aliases and snap policies.
    #[serde(default)]
    pub canonical_terms: Vec<CanonicalTerm>,
}

fn default_doc_version() -> u32 {
    SHARED_DOC_VERSION
}

impl SharedWordList {
    fn empty() -> Self {
        Self {
            version: SHARED_DOC_VERSION,
            entries: Vec::new(),
            canonical_terms: Vec::new(),
        }
    }
}

/// Parse and vet a remote document. Pure.
///
/// A missing `version` reads as 1; a higher version is refused; rows a
/// hand-edited file can carry that Thoth itself never stores (empty keys,
/// duplicate keys) are dropped, first occurrence winning.
fn parse_shared_doc(text: &str) -> Result<SharedWordList, String> {
    let doc: SharedWordList = serde_json::from_str(text)
        .map_err(|e| format!("The file at this URL is not a word list: {e}"))?;
    if doc.version > SHARED_DOC_VERSION {
        return Err(format!(
            "The shared word list is version {}, and this Thoth reads version {} \
             — update Thoth to read it",
            doc.version, SHARED_DOC_VERSION
        ));
    }
    Ok(SharedWordList {
        version: SHARED_DOC_VERSION,
        entries: sanitise_entries(doc.entries),
        canonical_terms: sanitise_terms(doc.canonical_terms),
    })
}

fn sanitise_entries(entries: Vec<DictionaryEntry>) -> Vec<DictionaryEntry> {
    let mut seen = HashSet::new();
    entries
        .into_iter()
        .filter(|e| {
            !e.from.trim().is_empty()
                && !e.to.trim().is_empty()
                && seen.insert(e.from.to_lowercase())
        })
        .collect()
}

fn sanitise_terms(terms: Vec<CanonicalTerm>) -> Vec<CanonicalTerm> {
    let mut seen = HashSet::new();
    terms
        .into_iter()
        .filter(|t| !t.term.trim().is_empty() && seen.insert(t.term.to_lowercase()))
        .collect()
}

// ---------------------------------------------------------------------------
// Three-way merge
// ---------------------------------------------------------------------------

/// Index rows by their key, last occurrence winning. The upstream lists are
/// deduplicated on save and [`parse_shared_doc`] deduplicates theirs, so the
/// last-wins choice only matters for a document that bypassed both.
fn index_by_key<T>(rows: &[T], key: &impl Fn(&T) -> String) -> HashMap<String, usize> {
    rows.iter().enumerate().map(|(i, r)| (key(r), i)).collect()
}

/// Three-way merge of one keyed section.
///
/// Returns the merged rows and the number of keys changed on both sides since
/// the base (conflicts). Order: ours first in ours order, then rows only
/// theirs had, in theirs order — dictionary application order is significant.
///
/// With no base (first sync, or the URL changed and orphaned the old base):
/// union, ours winning per key, no deletions.
fn merge_keyed<T: PartialEq + Clone>(
    base: Option<&[T]>,
    ours: &[T],
    theirs: &[T],
    key: impl Fn(&T) -> String,
) -> (Vec<T>, usize) {
    let Some(base) = base else {
        let mut merged = ours.to_vec();
        let mut seen: HashSet<String> = ours.iter().map(&key).collect();
        for row in theirs {
            if seen.insert(key(row)) {
                merged.push(row.clone());
            }
        }
        return (merged, 0);
    };

    let ours_idx = index_by_key(ours, &key);
    let theirs_idx = index_by_key(theirs, &key);
    let base_idx = index_by_key(base, &key);

    let mut merged = Vec::new();
    let mut conflicts = 0;
    let mut seen: HashSet<String> = HashSet::new();
    for row in ours.iter().chain(theirs) {
        let k = key(row);
        if !seen.insert(k.clone()) {
            continue;
        }
        let o = ours_idx.get(&k).map(|&i| &ours[i]);
        let t = theirs_idx.get(&k).map(|&i| &theirs[i]);
        let b = base_idx.get(&k).map(|&i| &base[i]);
        let chosen = if o == t {
            // Same row on both sides: nothing to do (None == None is covered).
            o
        } else if o == b {
            // Ours unchanged since the base: theirs wins, including its
            // absence — a remote deletion propagates.
            t
        } else if t == b {
            // Theirs unchanged since the base: ours wins, including its
            // absence — a local deletion propagates.
            o
        } else {
            // Changed on both sides: Thoth's version wins.
            conflicts += 1;
            o
        };
        if let Some(row) = chosen {
            merged.push(row.clone());
        }
    }
    (merged, conflicts)
}

/// Merge both sections of the shared document against a common base.
fn merge_shared(
    base: Option<&SharedWordList>,
    ours: &SharedWordList,
    theirs: &SharedWordList,
) -> (SharedWordList, usize) {
    let (entries, entry_conflicts) = merge_keyed(
        base.map(|b| b.entries.as_slice()),
        &ours.entries,
        &theirs.entries,
        |e| e.from.to_lowercase(),
    );
    let (canonical_terms, term_conflicts) = merge_keyed(
        base.map(|b| b.canonical_terms.as_slice()),
        &ours.canonical_terms,
        &theirs.canonical_terms,
        |t| t.term.to_lowercase(),
    );
    (
        SharedWordList {
            version: SHARED_DOC_VERSION,
            entries,
            canonical_terms,
        },
        entry_conflicts + term_conflicts,
    )
}

// ---------------------------------------------------------------------------
// WebDAV client
// ---------------------------------------------------------------------------

/// A one-URL WebDAV client: plain HTTP GET/PUT with basic auth, no locking.
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
            // Without a username the requests are anonymous and the server's
            // own 401 answers. A username with no stored password is reported
            // by `run_cycle` before any request is made.
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

    /// GET the shared file. `Ok(None)` means 404: nothing synced into this
    /// slot yet, which the cycle treats as an empty document.
    async fn fetch_doc(&self) -> Result<Option<SharedWordList>, String> {
        let response = tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.signed(crate::http_client().get(&self.url)).send(),
        )
        .await
        .map_err(|_| format!("Timed out after {}s", REQUEST_TIMEOUT.as_secs()))?
        .map_err(|e| format!("The GET failed: {e}"))?;
        match response.status() {
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            status if !status.is_success() => Err(format!("The server answered {status} for GET")),
            _ => {
                let text = tokio::time::timeout(REQUEST_TIMEOUT, response.text())
                    .await
                    .map_err(|_| format!("Timed out after {}s", REQUEST_TIMEOUT.as_secs()))?
                    .map_err(|e| format!("The GET failed: {e}"))?;
                parse_shared_doc(&text).map(Some)
            }
        }
    }

    /// PUT the merged document back.
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

/// Where the WebDAV password lives. Never config.json, never the transcript.
///
/// Release builds on Linux and Windows use the OS keyring, where a desktop
/// app's credentials belong. Debug builds use a file store so dev iteration
/// and test harnesses never touch the keyring; macOS uses the file store in
/// every build because the app is ad-hoc signed and the keychain's
/// partition-list gate would re-prompt on every rebuild (rules-library
/// `stacks/tauri.md`).
mod password_store {
    /// Keyring entry names (only referenced by the keyring backend, so they
    /// carry its cfg and never read as dead code in a file-store build).
    #[cfg(all(not(debug_assertions), any(target_os = "linux", target_os = "windows")))]
    const SERVICE: &str = "com.poodle64.thoth";
    #[cfg(all(not(debug_assertions), any(target_os = "linux", target_os = "windows")))]
    const ACCOUNT: &str = "word-list-sync";

    #[cfg(any(debug_assertions, not(any(target_os = "linux", target_os = "windows"))))]
    use std::{fs, path::PathBuf};

    #[cfg(any(debug_assertions, not(any(target_os = "linux", target_os = "windows"))))]
    fn file_path() -> PathBuf {
        crate::config::get_config_dir().join("sync_password")
    }

    // -- OS keyring (release, Linux/Windows) --------------------------------

    #[cfg(all(not(debug_assertions), any(target_os = "linux", target_os = "windows")))]
    pub fn set(password: Option<&str>) -> Result<(), String> {
        let entry = keyring::Entry::new(SERVICE, ACCOUNT)
            .map_err(|e| format!("Failed to open the OS keyring: {e}"))?;
        match password {
            Some(pw) => entry
                .set_password(pw)
                .map_err(|e| format!("Failed to store the password in the OS keyring: {e}")),
            None => match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(format!("Failed to clear the OS keyring entry: {e}")),
            },
        }
    }

    #[cfg(all(not(debug_assertions), any(target_os = "linux", target_os = "windows")))]
    pub fn get() -> Option<String> {
        keyring::Entry::new(SERVICE, ACCOUNT)
            .and_then(|entry| entry.get_password())
            .ok()
            .filter(|pw| !pw.is_empty())
    }

    // -- Owner-only file (debug builds; every build on macOS) ----------------

    #[cfg(any(debug_assertions, not(any(target_os = "linux", target_os = "windows"))))]
    pub fn set(password: Option<&str>) -> Result<(), String> {
        match password {
            Some(pw) => {
                let dir = crate::config::get_config_dir();
                fs::create_dir_all(&dir)
                    .map_err(|e| format!("Failed to create the config directory: {e}"))?;
                fs::write(file_path(), pw)
                    .map_err(|e| format!("Failed to write the password store: {e}"))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Err(e) =
                        fs::set_permissions(file_path(), fs::Permissions::from_mode(0o600))
                    {
                        tracing::warn!("Failed to set password-store permissions to 0o600: {e}");
                    }
                }
                Ok(())
            }
            None => match fs::remove_file(file_path()) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("Failed to clear the password store: {e}")),
            },
        }
    }

    #[cfg(any(debug_assertions, not(any(target_os = "linux", target_os = "windows"))))]
    pub fn get() -> Option<String> {
        fs::read_to_string(file_path())
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|pw| !pw.is_empty())
    }
}

// ---------------------------------------------------------------------------
// Persisted state and status
// ---------------------------------------------------------------------------

/// What sync persists between runs: the URL the base snapshot belongs to, and
/// the last document both sides agreed on. A URL change orphans the base, so
/// the next cycle falls back to the union merge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedState {
    url: String,
    last_synced: Option<SharedWordList>,
}

fn state_path() -> PathBuf {
    crate::config::get_config_dir().join("sync_state.json")
}

static PERSISTED: OnceLock<RwLock<Option<PersistedState>>> = OnceLock::new();

fn persisted_state() -> &'static RwLock<Option<PersistedState>> {
    PERSISTED.get_or_init(|| {
        let loaded = fs::read_to_string(state_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        RwLock::new(loaded)
    })
}

fn store_state(state: &PersistedState) -> Result<(), String> {
    let dir = crate::config::get_config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create the config directory: {e}"))?;
    let text =
        serde_json::to_string(state).map_err(|e| format!("Failed to serialise sync state: {e}"))?;
    fs::write(state_path(), text).map_err(|e| format!("Failed to write sync state: {e}"))?;
    *persisted_state().write() = Some(state.clone());
    Ok(())
}

/// What the Settings pane shows about sync. The password itself never appears.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub enabled: bool,
    pub url_set: bool,
    pub has_password: bool,
    /// RFC 3339 timestamp of the last successful sync, if any
    pub last_sync_at: Option<String>,
    /// The last cycle's failure, cleared by the next success
    pub last_error: Option<String>,
}

#[derive(Default, Clone)]
struct StatusInner {
    last_sync_at: Option<String>,
    last_error: Option<String>,
}

static STATUS: OnceLock<RwLock<StatusInner>> = OnceLock::new();

fn status() -> &'static RwLock<StatusInner> {
    STATUS.get_or_init(|| RwLock::new(StatusInner::default()))
}

fn record_success() {
    *status().write() = StatusInner {
        last_sync_at: Some(chrono::Utc::now().to_rfc3339()),
        last_error: None,
    };
}

fn record_error(message: &str) {
    status().write().last_error = Some(message.to_string());
}

// ---------------------------------------------------------------------------
// The sync cycle
// ---------------------------------------------------------------------------

/// Snapshot both lists as they stand on this machine.
fn capture_local() -> SharedWordList {
    SharedWordList {
        version: SHARED_DOC_VERSION,
        entries: crate::dictionary::get_dictionary_entries().unwrap_or_default(),
        canonical_terms: crate::canonical::get_canonical_terms().unwrap_or_default(),
    }
}

/// Write a merged document back into Thoth's own stores.
fn apply_local(doc: &SharedWordList) -> Result<(), String> {
    crate::dictionary::replace_all_entries(doc.entries.clone())
        .map_err(|e| format!("Failed to apply the synced dictionary: {e}"))?;
    crate::canonical::replace_all_terms(doc.canonical_terms.clone())
        .map_err(|e| format!("Failed to apply the synced canonical terms: {e}"))?;
    Ok(())
}

static CYCLE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn cycle_mutex() -> &'static tokio::sync::Mutex<()> {
    CYCLE.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// One cycle, serialised against the background loop and any manual run.
async fn cycle_guarded() -> Result<(), String> {
    let _guard = cycle_mutex().lock().await;
    run_cycle().await
}

/// One sync cycle: fetch, merge, apply, push, persist.
///
/// On failure the persisted base is left untouched, so the next cycle
/// re-merges against the same ancestor — converging rather than duplicating.
/// Errors are returned verbatim for `sync_now` and recorded into the status
/// by the caller.
async fn run_cycle() -> Result<(), String> {
    let cfg = crate::config::get_config().map_err(|e| format!("Cannot read the config: {e}"))?;
    let sync = &cfg.sync;
    let url = sync.url.trim().to_string();
    if url.is_empty() {
        return Err("Word list sync has no URL set".to_string());
    }
    let password = password_store::get();
    if !sync.username.trim().is_empty() && password.is_none() {
        return Err(
            "A username is set but no password is stored — enter the password in \
             Settings → Integrations"
                .to_string(),
        );
    }

    let client = WebDavClient::new(&url, sync.username.trim(), password.as_deref())?;
    let ours = capture_local();
    // A base belonging to another URL is not a common ancestor: drop it and
    // fall back to the union merge.
    let base = persisted_state()
        .read()
        .clone()
        .filter(|state| state.url == url)
        .and_then(|state| state.last_synced);
    let theirs = client
        .fetch_doc()
        .await?
        .unwrap_or_else(SharedWordList::empty);

    let (merged, conflicts) = merge_shared(base.as_ref(), &ours, &theirs);
    if conflicts > 0 {
        tracing::warn!(
            "Word list sync merged {conflicts} entries changed on both sides; kept \
             this Thoth's version"
        );
    }
    if merged != ours {
        apply_local(&merged)?;
    }
    if merged != theirs {
        client.put_doc(&merged).await?;
    }
    store_state(&PersistedState {
        url,
        last_synced: Some(merged),
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

static WAKE: OnceLock<tokio::sync::Notify> = OnceLock::new();
static LOCAL_DIRTY: AtomicBool = AtomicBool::new(false);

fn wake() -> &'static tokio::sync::Notify {
    WAKE.get_or_init(tokio::sync::Notify::new)
}

/// Tell sync a local list changed. Called from `save_dictionary` and
/// `save_registry`, the two funnels every mutation path — UI, MCP, import —
/// already goes through. Harmless when sync is off or no loop is running.
pub fn notify_local_change() {
    LOCAL_DIRTY.store(true, Ordering::Release);
    wake().notify_one();
}

/// The sync loop. A tick costs one config read; only a local change (≤ one
/// tick after it) or the 60-second poll boundary runs a cycle.
pub fn spawn_sync_loop() {
    tauri::async_runtime::spawn(async {
        let mut tick: u32 = 0;
        loop {
            tokio::select! {
                _ = wake().notified() => {}
                _ = tokio::time::sleep(TICK) => {}
            }
            tick = tick.wrapping_add(1);
            let dirty = LOCAL_DIRTY.swap(false, Ordering::AcqRel);
            // Pull at tick 1 (5 s after launch) and every PULL_EVERY_TICKS
            // ticks after; otherwise only a local change runs a cycle.
            if !dirty && tick % PULL_EVERY_TICKS != 1 {
                continue;
            }
            let cfg = match crate::config::get_config() {
                Ok(cfg) => cfg,
                Err(_) => continue,
            };
            if !cfg.sync.enabled || cfg.sync.url.trim().is_empty() {
                continue;
            }
            if let Err(e) = cycle_guarded().await {
                record_error(&e);
                tracing::warn!("Word list sync cycle failed: {e}");
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// The sync state the Settings pane shows. Never the password itself.
#[tauri::command]
pub fn get_sync_status() -> SyncStatus {
    tauri_plugin_telemetry::traced_sync_value("get_sync_status", || {
        let cfg = crate::config::get_config().unwrap_or_default();
        let inner = status().read().clone();
        SyncStatus {
            enabled: cfg.sync.enabled,
            url_set: !cfg.sync.url.trim().is_empty(),
            has_password: password_store::get().is_some(),
            last_sync_at: inner.last_sync_at,
            last_error: inner.last_error,
        }
    })
}

/// Store or clear (`None` or empty string) the WebDAV password. Write-only:
/// no command ever returns it.
#[tauri::command]
pub fn set_sync_password(password: Option<String>) -> Result<(), Error> {
    tauri_plugin_telemetry::traced_sync("set_sync_password", || {
        let password = password.filter(|pw| !pw.is_empty());
        password_store::set(password.as_deref()).map_err(Error::from)
    })
}

/// Run one sync cycle now and report the outcome. Runs the same merge the
/// background loop runs, serialised against it; the outcome (including any
/// failure) lands in the returned status rather than the command error, so
/// the Settings pane can show it.
#[tauri::command]
pub async fn sync_now() -> Result<SyncStatus, Error> {
    tauri_plugin_telemetry::traced("sync_now", async move {
        match cycle_guarded().await {
            Ok(()) => record_success(),
            Err(e) => record_error(&e),
        }
        Ok(get_sync_status())
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

    fn term(name: &str) -> CanonicalTerm {
        CanonicalTerm {
            term: name.to_string(),
            aliases: vec![],
            policy: SnapPolicy::AliasOnly,
            max_words: 3,
            threshold: None,
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
    // Three-way merge, dictionary section
    // -------------------------------------------------------------------------

    #[test]
    fn remote_addition_edit_and_deletion_propagate() {
        let base = doc(vec![entry("teh", "the"), entry("old", "kept")], vec![]);
        let ours = doc(vec![entry("teh", "the"), entry("old", "kept")], vec![]);
        // Remote: added "gonna", changed "teh", deleted "old".
        let theirs = doc(
            vec![entry("teh", "thee"), entry("gonna", "going to")],
            vec![],
        );

        let (merged, conflicts) = merge_shared(Some(&base), &ours, &theirs);
        assert_eq!(conflicts, 0);
        assert_eq!(
            merged.entries,
            vec![entry("teh", "thee"), entry("gonna", "going to")]
        );
    }

    #[test]
    fn local_addition_edit_and_deletion_propagate() {
        let base = doc(vec![entry("teh", "the")], vec![]);
        let ours = doc(vec![entry("teh", "thee"), entry("mine", "yours")], vec![]);
        let theirs = doc(vec![entry("teh", "the")], vec![]);

        let (merged, conflicts) = merge_shared(Some(&base), &ours, &theirs);
        assert_eq!(conflicts, 0);
        assert_eq!(merged.entries, ours.entries);
    }

    #[test]
    fn same_entry_changed_on_both_sides_keeps_ours() {
        let base = doc(vec![entry("teh", "the")], vec![]);
        let ours = doc(vec![entry("teh", "ours")], vec![]);
        let theirs = doc(vec![entry("teh", "theirs")], vec![]);

        let (merged, conflicts) = merge_shared(Some(&base), &ours, &theirs);
        assert_eq!(conflicts, 1);
        assert_eq!(merged.entries, vec![entry("teh", "ours")]);
    }

    #[test]
    fn remote_deletion_of_locally_edited_entry_is_a_conflict_kept_ours() {
        let base = doc(vec![entry("teh", "the")], vec![]);
        let ours = doc(vec![entry("teh", "thee")], vec![]);
        let theirs = doc(vec![], vec![]);

        let (merged, conflicts) = merge_shared(Some(&base), &ours, &theirs);
        assert_eq!(conflicts, 1);
        assert_eq!(merged.entries, vec![entry("teh", "thee")]);
    }

    #[test]
    fn local_deletion_of_remotely_edited_entry_is_a_conflict_stays_deleted() {
        let base = doc(vec![entry("teh", "the")], vec![]);
        let ours = doc(vec![], vec![]);
        let theirs = doc(vec![entry("teh", "thee")], vec![]);

        let (merged, conflicts) = merge_shared(Some(&base), &ours, &theirs);
        assert_eq!(conflicts, 1);
        assert_eq!(merged.entries, vec![]);
    }

    #[test]
    fn no_base_takes_union_with_ours_winning_and_no_deletions() {
        let ours = doc(vec![entry("teh", "the"), entry("mine", "mine's")], vec![]);
        let theirs = doc(
            vec![entry("teh", "their's"), entry("theirs", "new")],
            vec![term("LiteLLM")],
        );

        let (merged, conflicts) = merge_shared(None, &ours, &theirs);
        assert_eq!(conflicts, 0);
        // Ours order first, then theirs-new in theirs order; ours wins the key.
        assert_eq!(
            merged.entries,
            vec![
                entry("teh", "the"),
                entry("mine", "mine's"),
                entry("theirs", "new")
            ]
        );
        assert_eq!(merged.canonical_terms, vec![term("LiteLLM")]);
    }

    #[test]
    fn merge_keyed_keys_are_case_insensitive() {
        let base = doc(vec![entry("Teh", "the")], vec![]);
        let ours = doc(vec![], vec![]); // local deleted "Teh"
        let theirs = doc(vec![entry("teh", "the")], vec![]); // remote unchanged

        let (merged, _) = merge_shared(Some(&base), &ours, &theirs);
        assert_eq!(merged.entries, vec![]);
    }

    #[test]
    fn canonical_terms_merge_their_own_section() {
        let base = doc(vec![], vec![term("LiteLLM")]);
        let ours = doc(vec![], vec![term("LiteLLM")]);
        let theirs = doc(
            vec![],
            vec![CanonicalTerm {
                term: "LiteLLM".to_string(),
                aliases: vec!["lite llm".to_string()],
                policy: SnapPolicy::Phonetic,
                max_words: 2,
                threshold: None,
            }],
        );

        let (merged, conflicts) = merge_shared(Some(&base), &ours, &theirs);
        assert_eq!(conflicts, 0);
        assert_eq!(merged.canonical_terms, theirs.canonical_terms);
    }

    // -------------------------------------------------------------------------
    // Document parsing
    // -------------------------------------------------------------------------

    #[test]
    fn round_trip_preserves_rows() {
        let original = doc(
            vec![entry("teh", "the")],
            vec![CanonicalTerm {
                term: "LiteLLM".to_string(),
                aliases: vec!["lite llm".to_string()],
                policy: SnapPolicy::Phonetic,
                max_words: 2,
                threshold: Some(0.6),
            }],
        );
        let text = serde_json::to_string(&original).unwrap();
        assert_eq!(parse_shared_doc(&text).unwrap(), original);
    }

    #[test]
    fn serialised_field_names_are_camelcase_for_second_consumers() {
        let text = serde_json::to_string(&doc(vec![entry("teh", "the")], vec![])).unwrap();
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

    #[test]
    fn missing_version_reads_as_one() {
        let parsed = parse_shared_doc(r#"{"entries":[]}"#).unwrap();
        assert_eq!(parsed.version, 1);
    }

    #[test]
    fn higher_version_is_refused() {
        let err = parse_shared_doc(r#"{"version": 2, "entries": []}"#).unwrap_err();
        assert!(err.contains("version 2"));
    }

    #[test]
    fn empty_rows_and_duplicates_are_dropped_first_wins() {
        let parsed = parse_shared_doc(
            r#"{"entries": [
                {"from": "teh", "to": "the"},
                {"from": "TEH", "to": "second"},
                {"from": "", "to": "x"},
                {"from": "x", "to": ""}
            ], "canonicalTerms": [
                {"term": "LiteLLM", "aliases": [], "policy": "aliasOnly", "maxWords": 2, "threshold": null},
                {"term": "litellm", "aliases": [], "policy": "aliasOnly", "maxWords": 2, "threshold": null}
            ]}"#,
        )
        .unwrap();
        assert_eq!(parsed.entries, vec![entry("teh", "the")]);
        assert_eq!(
            parsed.canonical_terms,
            vec![CanonicalTerm {
                term: "LiteLLM".to_string(),
                aliases: vec![],
                policy: SnapPolicy::AliasOnly,
                max_words: 2,
                threshold: None,
            }]
        );
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

    // -------------------------------------------------------------------------
    // WebDAV over mockito
    // -------------------------------------------------------------------------

    fn file_url(server: &mockito::ServerGuard) -> String {
        format!("{}/words.json", server.url())
    }

    #[tokio::test]
    async fn fetch_parses_a_valid_document() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/words.json")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"version":1,"entries":[{"from":"teh","to":"the","caseSensitive":false}]}"#,
            )
            .create_async()
            .await;

        let client = WebDavClient::new(&file_url(&server), "", None).unwrap();
        let doc = client.fetch_doc().await.unwrap().unwrap();
        assert_eq!(doc.entries, vec![entry("teh", "the")]);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn fetch_404_is_an_empty_slot_not_an_error() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/words.json")
            .with_status(404)
            .create_async()
            .await;

        let client = WebDavClient::new(&file_url(&server), "", None).unwrap();
        assert!(client.fetch_doc().await.unwrap().is_none());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn fetch_error_status_surfaces_the_status() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/words.json")
            .with_status(401)
            .create_async()
            .await;

        let client = WebDavClient::new(&file_url(&server), "", None).unwrap();
        let err = client.fetch_doc().await.unwrap_err();
        assert!(err.contains("401"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn fetch_sends_basic_auth_when_configured() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/words.json")
            .match_header("authorization", "Basic dXNlcjpwYXNz")
            .with_status(200)
            .with_body(r#"{"version":1,"entries":[]}"#)
            .create_async()
            .await;

        let client = WebDavClient::new(&file_url(&server), "user", Some("pass")).unwrap();
        assert!(client.fetch_doc().await.unwrap().is_some());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn put_writes_the_merged_document() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("PUT", "/words.json")
            .match_header("content-type", "application/json")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"version":1,"entries":[{"from":"teh","to":"the","caseSensitive":false}]}"#
                    .to_string(),
            ))
            .with_status(201)
            .create_async()
            .await;

        let client = WebDavClient::new(&file_url(&server), "", None).unwrap();
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
        let err = client.put_doc(&SharedWordList::empty()).await.unwrap_err();
        assert!(err.contains("403"), "unexpected error: {err}");
    }

    // The password store is deliberately untested here: it writes the real
    // `~/.thoth`, which `get_config_dir()` points at the operator's home with
    // no test override, and touching a real stored credential from a test is
    // worse than not testing a ten-line file write.
}
