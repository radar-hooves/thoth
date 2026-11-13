//! External dictionary: the household's wordhoard service as the source of
//! truth for both lists, instead of Thoth publishing its own file.
//!
//! Off by default (a public build with the setting untouched never touches the
//! network). When on, the service's two lists replace the local ones:
//!
//! - **Read** `GET {url}/api/dictionary/word-list` on start, on a timer and
//!   after every write, with `If-None-Match` (304 = unchanged).
//! - **Write through** every local edit as a batch to
//!   `POST {url}/api/dictionary/changes`, the device named in
//!   `X-Wordhoard-Device`. The batch is the diff between the last good list and
//!   the local lists, so an edit made offline is sent when the service returns.
//! - **Offline** the last good document stays on disk and is the local list;
//!   a failed request records the error and changes nothing.
//!
//! Basic auth with the Mac's own app password, held in an owner-only file like
//! the WebDAV one. The wire contract is in `docs/word-list-publish.md`.

use crate::canonical::CanonicalTerm;
use crate::error::Error;
use crate::publish::{SharedWordList, basic_auth_header, read_secret, write_secret};
use parking_lot::RwLock;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

const HELPER_TIMEOUT: Duration = Duration::from_secs(20);
const PASSWORD_FILE: &str = "wordhoard_password";
const LAST_GOOD_FILE: &str = "external_dictionary_last_good.json";
const BEFORE_FILE: &str = "dictionary_before_external.json";
const DEBOUNCE: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const VERSION_HEADER: &str = "X-Wordhoard-Device";

// ---------------------------------------------------------------------------
// Diff: last good list -> local lists, as service changes
// ---------------------------------------------------------------------------

/// The changes that turn `base` into `local`: removes first, then adds, so a
/// changed row is written as remove-old, add-new.
fn diff(base: &SharedWordList, local: &SharedWordList) -> Vec<Value> {
    let mut changes = Vec::new();
    for e in base.entries.iter().filter(|e| !local.entries.contains(e)) {
        changes.push(json!({"kind":"entry","op":"remove","from":e.from,"to":e.to,"caseSensitive":e.case_sensitive}));
    }
    for t in &base.canonical_terms {
        if !local.canonical_terms.iter().any(|l| l.term == t.term) {
            changes.push(json!({"kind":"term","op":"remove","term":t.term}));
        }
    }
    for e in local.entries.iter().filter(|e| !base.entries.contains(e)) {
        changes.push(json!({"kind":"entry","op":"add","from":e.from,"to":e.to,"caseSensitive":e.case_sensitive}));
    }
    for t in local
        .canonical_terms
        .iter()
        .filter(|t| !base.canonical_terms.contains(t))
    {
        changes.push(term_add(t));
    }
    changes
}

fn term_add(t: &CanonicalTerm) -> Value {
    json!({
        "kind":"term","op":"add","term":t.term,"aliases":t.aliases,
        "policy":t.policy,"maxWords":t.max_words,"threshold":t.threshold,
    })
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

struct Client {
    base: String,
    auth: String,
    device: String,
}

impl Client {
    fn new(url: &str, username: &str, password: &str, device: &str) -> Result<Self, String> {
        let parsed = url::Url::parse(url.trim()).map_err(|e| format!("Not a valid URL: {e}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(format!(
                "The URL must be http or https, not {}",
                parsed.scheme()
            ));
        }
        Ok(Self {
            base: url.trim().trim_end_matches('/').to_string(),
            auth: basic_auth_header(username, password),
            device: device.to_string(),
        })
    }

    /// `Ok(None)` is a 304; `Ok(Some((doc, etag)))` a 200.
    async fn fetch(&self, etag: Option<&str>) -> Result<Option<(SharedWordList, String)>, String> {
        let mut req = crate::http_client()
            .get(format!("{}/api/dictionary/word-list", self.base))
            .header("Authorization", &self.auth);
        if let Some(tag) = etag {
            req = req.header("If-None-Match", tag);
        }
        let resp = tokio::time::timeout(REQUEST_TIMEOUT, req.send())
            .await
            .map_err(|_| "The dictionary service timed out".to_string())?
            .map_err(|e| format!("The dictionary service is unreachable: {e}"))?;
        match resp.status().as_u16() {
            304 => Ok(None),
            200 => {
                let tag = resp
                    .headers()
                    .get("etag")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                let doc: SharedWordList = resp
                    .json()
                    .await
                    .map_err(|e| format!("The word list is not a version-1 document: {e}"))?;
                if doc.version != 1 {
                    return Err(format!(
                        "The word list is version {}; Thoth reads version 1",
                        doc.version
                    ));
                }
                Ok(Some((doc, tag)))
            }
            s => Err(format!(
                "The dictionary service answered {s} for the word list"
            )),
        }
    }

    async fn post(&self, changes: Vec<Value>) -> Result<(), String> {
        let req = crate::http_client()
            .post(format!("{}/api/dictionary/changes", self.base))
            .header("Authorization", &self.auth)
            .header(VERSION_HEADER, &self.device)
            .json(&json!({ "changes": changes }));
        let resp = tokio::time::timeout(REQUEST_TIMEOUT, req.send())
            .await
            .map_err(|_| "The dictionary service timed out".to_string())?
            .map_err(|e| format!("The dictionary service is unreachable: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!(
                "The dictionary service answered {} for the changes",
                resp.status()
            ));
        }
        Ok(())
    }
}

/// What one sync did.
#[derive(Debug)]
struct Outcome {
    /// The local lists were sent (so `base` is now `local`).
    pushed: bool,
    /// The service's newer document, if it had one.
    fetched: Option<(SharedWordList, String)>,
}

/// Push the local edits since `base`, then read. A failed push returns before
/// the read, so the local edits are neither lost nor overwritten.
async fn reconcile(
    client: &Client,
    base: Option<&SharedWordList>,
    local: &SharedWordList,
    etag: Option<&str>,
) -> Result<Outcome, String> {
    let mut pushed = false;
    let mut etag = etag;
    if let Some(base) = base {
        let changes = diff(base, local);
        if !changes.is_empty() {
            client.post(changes).await?;
            pushed = true;
            // The service has a new document now; the old tag is stale.
            etag = None;
        }
    }
    let fetched = client.fetch(etag).await?;
    Ok(Outcome { pushed, fetched })
}

// ---------------------------------------------------------------------------
// Last good document
// ---------------------------------------------------------------------------

fn last_good_path() -> PathBuf {
    crate::config::get_config_dir().join(LAST_GOOD_FILE)
}

fn load_last_good(path: &Path) -> Option<SharedWordList> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn store_doc(path: &Path, doc: &SharedWordList) -> Result<(), String> {
    let body = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    std::fs::write(path, body).map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Status and the sync
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalDictionaryStatus {
    pub has_password: bool,
    /// RFC 3339 time of the last successful sync
    pub last_sync_at: Option<String>,
    /// The last sync's failure, cleared by the next success. While set, the
    /// local lists are the last good copy.
    pub last_error: Option<String>,
}

#[derive(Default)]
struct State {
    last_sync_at: Option<String>,
    last_error: Option<String>,
    etag: Option<String>,
}

static STATE: OnceLock<RwLock<State>> = OnceLock::new();
fn state() -> &'static RwLock<State> {
    STATE.get_or_init(|| RwLock::new(State::default()))
}

static SYNC: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
static WAKE: OnceLock<tokio::sync::Notify> = OnceLock::new();
fn wake() -> &'static tokio::sync::Notify {
    WAKE.get_or_init(tokio::sync::Notify::new)
}

/// A local list changed; write it through. Cheap and harmless when the mode is off.
pub fn notify_local_change() {
    wake().notify_one();
}

pub fn is_enabled() -> bool {
    crate::config::get_config()
        .map(|c| c.external_dictionary.enabled)
        .unwrap_or(false)
}

fn device_name(configured: &str) -> String {
    let configured = configured.trim();
    if !configured.is_empty() {
        return configured.to_string();
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "thoth".to_string())
}

fn capture_local() -> SharedWordList {
    SharedWordList {
        version: 1,
        entries: crate::dictionary::get_dictionary_entries().unwrap_or_default(),
        canonical_terms: crate::canonical::get_canonical_terms().unwrap_or_default(),
    }
}

/// The app password: from the broker through the credential helper when one is
/// set (held in memory only, never written), else the manually entered one.
async fn resolve_password(helper: &str) -> Result<String, String> {
    let helper = helper.trim();
    if helper.is_empty() {
        return read_secret(PASSWORD_FILE).ok_or_else(|| {
            "No password is stored — enter it in Settings → Integrations".to_string()
        });
    }
    run_helper(helper, HELPER_TIMEOUT).await
}

/// Run the helper under `sh -c` and take its trimmed stdout. Its output and
/// stderr never reach an error, a log or the status: they may be the credential.
async fn run_helper(command: &str, timeout: Duration) -> Result<String, String> {
    let child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(timeout, child)
        .await
        .map_err(|_| {
            format!(
                "The credential helper took longer than {}s",
                timeout.as_secs()
            )
        })?
        .map_err(|e| format!("The credential helper could not run: {e}"))?;
    if !out.status.success() {
        return Err(format!("The credential helper failed ({})", out.status));
    }
    let secret = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if secret.is_empty() {
        return Err("The credential helper printed nothing".to_string());
    }
    Ok(secret)
}

/// One sync. `require_ready` turns "mode off" into an error for the manual command.
async fn sync_once(require_ready: bool) -> Result<(), String> {
    let _guard = SYNC
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let cfg = crate::config::get_config().map_err(|e| format!("Cannot read the config: {e}"))?;
    let ext = &cfg.external_dictionary;
    if !ext.enabled || ext.url.trim().is_empty() {
        if require_ready {
            return Err("The external dictionary is off, or has no URL set".to_string());
        }
        return Ok(());
    }
    let password = resolve_password(&ext.credential_helper).await?;
    let client = Client::new(
        &ext.url,
        ext.username.trim(),
        &password,
        &device_name(&ext.device),
    )?;

    let path = last_good_path();
    let base = load_last_good(&path);
    let local = capture_local();
    if base.is_none() {
        // First adoption replaces the local lists: keep what they were.
        let _ = store_doc(&crate::config::get_config_dir().join(BEFORE_FILE), &local);
    }
    let etag = if base.is_some() {
        state().read().etag.clone()
    } else {
        None
    };
    let outcome = reconcile(&client, base.as_ref(), &local, etag.as_deref()).await?;

    let (doc, tag) = match outcome.fetched {
        Some(fetched) => fetched,
        None => {
            if outcome.pushed {
                store_doc(&path, &local)?;
            }
            return record_ok();
        }
    };
    // Last good first: if the replace fails halfway, the next sync pushes no
    // phantom diff, it just reads again.
    store_doc(&path, &doc)?;
    crate::dictionary::replace_entries(doc.entries.clone()).map_err(|e| e.to_string())?;
    crate::canonical::replace_terms(doc.canonical_terms).map_err(|e| e.to_string())?;
    state().write().etag = Some(tag);
    record_ok()
}

fn record_ok() -> Result<(), String> {
    let mut s = state().write();
    s.last_sync_at = Some(chrono::Utc::now().to_rfc3339());
    s.last_error = None;
    Ok(())
}

fn record_error(message: &str) {
    state().write().last_error = Some(message.to_string());
}

/// The sync loop: once at start, then on a local save (after a short debounce)
/// or the poll timer, whichever comes first.
pub fn spawn_sync_loop() {
    tauri::async_runtime::spawn(async {
        loop {
            if let Err(e) = sync_once(false).await {
                record_error(&e);
                tracing::warn!("External dictionary sync failed: {e}");
            }
            tokio::select! {
                _ = wake().notified() => tokio::time::sleep(DEBOUNCE).await,
                _ = tokio::time::sleep(POLL) => {}
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

fn cfg_helper_empty() -> bool {
    crate::config::get_config()
        .map(|c| c.external_dictionary.credential_helper.trim().is_empty())
        .unwrap_or(true)
}

#[tauri::command]
pub fn get_external_dictionary_status() -> ExternalDictionaryStatus {
    let s = state().read();
    ExternalDictionaryStatus {
        has_password: !cfg_helper_empty() || read_secret(PASSWORD_FILE).is_some(),
        last_sync_at: s.last_sync_at.clone(),
        last_error: s.last_error.clone(),
    }
}

/// Store or clear the app password. Write-only.
#[tauri::command]
pub fn set_external_dictionary_password(password: Option<String>) -> Result<(), Error> {
    let password = password.filter(|pw| !pw.is_empty());
    write_secret(PASSWORD_FILE, password.as_deref()).map_err(Error::from)
}

/// Sync now; the outcome lands in the returned status.
#[tauri::command]
pub async fn sync_external_dictionary_now() -> Result<ExternalDictionaryStatus, Error> {
    if let Err(e) = sync_once(true).await {
        record_error(&e);
    }
    Ok(get_external_dictionary_status())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::SnapPolicy;
    use crate::dictionary::DictionaryEntry;

    fn entry(from: &str, to: &str) -> DictionaryEntry {
        DictionaryEntry {
            from: from.into(),
            to: to.into(),
            case_sensitive: false,
        }
    }
    fn term(t: &str) -> CanonicalTerm {
        CanonicalTerm {
            term: t.into(),
            aliases: vec![],
            policy: SnapPolicy::AliasOnly,
            max_words: 3,
            threshold: None,
        }
    }
    fn doc(entries: Vec<DictionaryEntry>, terms: Vec<CanonicalTerm>) -> SharedWordList {
        SharedWordList {
            version: 1,
            entries,
            canonical_terms: terms,
        }
    }
    fn client(server: &mockito::ServerGuard) -> Client {
        Client::new(&server.url(), "u", "p", "huginn").unwrap()
    }
    const DOC: &str = r#"{"version":1,"entries":[{"from":"teh","to":"the"}],"canonicalTerms":[{"term":"LiteLLM","aliases":["lite llm"]}]}"#;

    #[test]
    fn diff_is_removes_then_adds_and_empty_when_equal() {
        let base = doc(vec![entry("a", "b")], vec![term("X")]);
        assert!(diff(&base, &base.clone()).is_empty());
        let local = doc(vec![entry("a", "c")], vec![term("Y")]);
        let ops: Vec<_> = diff(&base, &local)
            .iter()
            .map(|c| {
                format!(
                    "{}:{}",
                    c["kind"].as_str().unwrap(),
                    c["op"].as_str().unwrap()
                )
            })
            .collect();
        assert_eq!(
            ops,
            ["entry:remove", "term:remove", "entry:add", "term:add"]
        );
    }

    #[tokio::test]
    async fn read_200_parses_the_v1_document_and_keeps_the_etag() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/dictionary/word-list")
            .match_header("authorization", "Basic dTpw")
            .with_status(200)
            .with_header("etag", "\"abc\"")
            .with_body(DOC)
            .create_async()
            .await;
        let (d, tag) = client(&server).fetch(None).await.unwrap().unwrap();
        assert_eq!(tag, "\"abc\"");
        assert_eq!(d.entries[0].to, "the");
        assert!(!d.entries[0].case_sensitive);
        assert_eq!(d.canonical_terms[0].aliases, ["lite llm"]);
    }

    #[tokio::test]
    async fn read_304_sends_if_none_match_and_is_unchanged() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/api/dictionary/word-list")
            .match_header("if-none-match", "\"abc\"")
            .with_status(304)
            .create_async()
            .await;
        assert!(
            client(&server)
                .fetch(Some("\"abc\""))
                .await
                .unwrap()
                .is_none()
        );
        m.assert_async().await;
    }

    #[tokio::test]
    async fn a_higher_version_is_refused() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/dictionary/word-list")
            .with_status(200)
            .with_body(r#"{"version":2,"entries":[],"canonicalTerms":[]}"#)
            .create_async()
            .await;
        let err = client(&server).fetch(None).await.unwrap_err();
        assert!(err.contains("version 2"), "{err}");
    }

    #[tokio::test]
    async fn an_edit_is_written_through_with_the_device_then_read_back() {
        let mut server = mockito::Server::new_async().await;
        let post = server
            .mock("POST", "/api/dictionary/changes")
            .match_header("x-wordhoard-device", "huginn")
            .match_header("authorization", "Basic dTpw")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"changes":[{"kind":"entry","op":"add","from":"foo","to":"bar"}]}"#.into(),
            ))
            .with_status(200)
            .with_body(r#"{"added":1,"changed":0,"removed":0,"unchanged":0}"#)
            .create_async()
            .await;
        let get = server
            .mock("GET", "/api/dictionary/word-list")
            .with_status(200)
            .with_header("etag", "\"n\"")
            .with_body(DOC)
            .create_async()
            .await;
        let base = doc(vec![], vec![]);
        let local = doc(vec![entry("foo", "bar")], vec![]);
        let out = reconcile(&client(&server), Some(&base), &local, Some("\"old\""))
            .await
            .unwrap();
        assert!(out.pushed && out.fetched.is_some());
        post.assert_async().await;
        get.assert_async().await;
    }

    #[tokio::test]
    async fn a_failed_write_aborts_before_the_read() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/api/dictionary/changes")
            .with_status(403)
            .create_async()
            .await;
        let get = server
            .mock("GET", "/api/dictionary/word-list")
            .expect(0)
            .create_async()
            .await;
        let err = reconcile(
            &client(&server),
            Some(&doc(vec![], vec![])),
            &doc(vec![entry("a", "b")], vec![]),
            None,
        )
        .await
        .unwrap_err();
        assert!(err.contains("403"), "{err}");
        get.assert_async().await;
    }

    #[tokio::test]
    async fn the_helper_vends_the_password_and_its_failures_name_no_output() {
        let t = Duration::from_secs(5);
        assert_eq!(
            run_helper("printf 'hunter2\\n'", t).await.unwrap(),
            "hunter2"
        );
        let err = run_helper("echo leaked; exit 3", t).await.unwrap_err();
        assert!(err.contains("failed") && !err.contains("leaked"), "{err}");
        assert!(run_helper("true", t).await.unwrap_err().contains("nothing"));
        let slow = run_helper("sleep 5", Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(slow.contains("longer than"), "{slow}");
    }

    #[tokio::test]
    async fn a_set_helper_is_the_only_source() {
        // Whatever is in the manual file, a failing helper is an error, not a fallback.
        assert!(resolve_password("exit 1").await.is_err());
        assert_eq!(resolve_password("echo vended").await.unwrap(), "vended");
    }

    #[tokio::test]
    async fn offline_errors_and_the_last_good_file_survives_a_restart() {
        // Nothing listens on port 1: the read fails, the caller changes nothing.
        let c = Client::new("http://127.0.0.1:1", "u", "p", "huginn").unwrap();
        assert!(
            reconcile(&c, None, &doc(vec![], vec![]), None)
                .await
                .is_err()
        );

        let dir = std::env::temp_dir().join(format!("thoth-ext-dict-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(LAST_GOOD_FILE);
        assert!(load_last_good(&path).is_none());
        store_doc(
            &path,
            &doc(vec![entry("teh", "the")], vec![term("LiteLLM")]),
        )
        .unwrap();
        let back = load_last_good(&path).unwrap();
        assert_eq!(back.entries[0].from, "teh");
        assert_eq!(back.canonical_terms[0].term, "LiteLLM");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The proof against the real service: add a harmless term, read it back,
    /// remove it. Run with the device's login in WORDHOARD_USER / WORDHOARD_PASSWORD
    /// (and WORDHOARD_URL), then read `/api/dictionary/history` for owner and device.
    #[tokio::test]
    #[ignore = "needs the live service and a device login"]
    async fn live_add_then_remove_a_test_term() {
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} not set"));
        let c = Client::new(
            &var("WORDHOARD_URL"),
            &var("WORDHOARD_USER"),
            &var("WORDHOARD_PASSWORD"),
            "thoth-live-proof",
        )
        .unwrap();
        let probe = term("ThothLiveProbeTerm");
        c.post(vec![term_add(&probe)]).await.unwrap();
        let (d, _) = c.fetch(None).await.unwrap().unwrap();
        assert!(d.canonical_terms.iter().any(|t| t.term == probe.term));
        c.post(vec![json!({"kind":"term","op":"remove","term":probe.term})])
            .await
            .unwrap();
        let (d, _) = c.fetch(None).await.unwrap().unwrap();
        assert!(!d.canonical_terms.iter().any(|t| t.term == probe.term));
    }
}
