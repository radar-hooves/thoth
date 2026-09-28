//! Local Control API — loopback HTTP server for external integrations
//!
//! Binds ONLY to 127.0.0.1 (never 0.0.0.0). Protected by static bearer-token auth.
//! Enabled by default; toggle via `integrations.apiEnabled`. The bearer token is
//! held in a dedicated, reset-proof store (see [`token_store`]), not config.json.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use uuid::Uuid;

use crate::TELEMETRY_TARGET;
use crate::error::Error;

pub(crate) mod token_store;

// ---------------------------------------------------------------------------
// Token generation
// ---------------------------------------------------------------------------

/// Token prefix — `sk-thoth-` marks it as a Thoth secret key (mirrors the
/// recognisable `sk-`/`sk-ant-` convention; greppable for secret scanners).
const TOKEN_PREFIX: &str = "sk-thoth-";

/// base62 alphabet (URL-safe, no ambiguous separators).
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Generate a new bearer token of the canonical secret-key shape:
/// `sk-thoth-<40 base62 chars>` from the OS CSPRNG.
pub fn generate_token() -> String {
    // 40 chars of base62 ≈ 238 bits of entropy. Reject-sample to avoid modulo bias.
    let mut secret = String::with_capacity(40);
    let mut buf = [0u8; 64];
    while secret.len() < 40 {
        getrandom::fill(&mut buf).expect("OS CSPRNG unavailable");
        for &b in buf.iter() {
            if (b as usize) < 248 {
                // 248 = 4 * 62; keeps the distribution uniform across the 62 symbols
                secret.push(BASE62[(b % 62) as usize] as char);
                if secret.len() == 40 {
                    break;
                }
            }
        }
    }
    format!("{}{}", TOKEN_PREFIX, secret)
}

// ---------------------------------------------------------------------------
// Server state / handle
// ---------------------------------------------------------------------------

/// Handle that can signal the running server to stop.
struct ServerHandle {
    abort: tokio::task::AbortHandle,
}

/// Global server handle — Some while the server is running.
static SERVER: Mutex<Option<ServerHandle>> = Mutex::const_new(None);

// ---------------------------------------------------------------------------
// Shared state passed into axum handlers
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ApiState {
    app_version: String,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum AppError {
    Internal(String),
    NotFound(String),
    BadRequest(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Internal(msg) => {
                let body = serde_json::json!({ "error": msg });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
            }
            AppError::NotFound(msg) => {
                let body = serde_json::json!({ "error": msg });
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            AppError::BadRequest(msg) => {
                let body = serde_json::json!({ "error": msg });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
        }
    }
}

// AppError::Internal is the catch-all for `?` on Result<_, String> and similar.
// NotFound / BadRequest must be constructed explicitly.
impl From<String> for AppError {
    fn from(e: String) -> Self {
        AppError::Internal(e)
    }
}

impl From<&str> for AppError {
    fn from(e: &str) -> Self {
        AppError::Internal(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Internal(e.to_string())
    }
}

// Tauri commands now return the crate-level `Error`; the HTTP handlers call
// those same functions, so map their error into a 500 via its Display string
// (identical to the previous `String` payload).
impl From<Error> for AppError {
    fn from(e: Error) -> Self {
        AppError::Internal(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Async transcription job registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscribeJob {
    status: String,
    transcript: Option<String>,
    error: Option<String>,
    /// When the job was submitted. Drives both expiry and cap eviction.
    #[serde(skip)]
    created: Instant,
}

/// Status of a job whose transcript has aged out of the registry.
///
/// Distinct from a 404 on purpose: a caller polling a long-finished job learns
/// the real reason rather than being told the id never existed (#172, same
/// principle as the path error in #118).
const JOB_STATUS_EXPIRED: &str = "expired";

/// How long a job's transcript stays readable after submission.
///
/// Jobs exist to be polled by `transcribe_status` shortly after submission, so an
/// hour is far past any real polling window while still bounding how long a
/// transcript sits in memory.
const JOB_TTL: Duration = Duration::from_secs(60 * 60);

/// Hard cap on registry entries, after which the oldest finished jobs are dropped
/// outright.
///
/// An expired entry is a ~100-byte tombstone, so this is a bound on the count
/// rather than a meaningful memory budget: it exists so a process running for
/// months cannot accumulate ids without limit. In-flight jobs are never evicted
/// — their completion handler writes back by id, and dropping the entry would
/// discard the transcript silently — so the registry can briefly exceed the cap
/// if that many jobs are submitted at once.
const MAX_JOBS: usize = 512;

/// In-memory job store for async file transcription.
static JOBS: Mutex<Option<HashMap<String, TranscribeJob>>> = Mutex::const_new(None);

/// Expire aged-out transcripts and enforce [`MAX_JOBS`].
///
/// Runs on submission rather than on a timer: a background sweep would be a
/// thread that exists only to tidy a map nobody is reading, and the registry only
/// grows when a job is submitted.
/// Whether a job has reached a terminal status and is therefore safe to expire or
/// evict.
///
/// Both the TTL and the cap key off this. A job that is still `queued` or
/// `processing` must survive both: transcription is deliberately starvable — the
/// background priority in `transcription::gate` stands aside for live dictation
/// indefinitely — so a job outliving the TTL while genuinely still running is a
/// normal state, not a stale one. Expiring it would report `expired` with a null
/// transcript for work that is about to succeed, and would then make it eligible
/// for cap eviction, at which point its completion handler's lookup by id finds
/// nothing and the real transcript is discarded in silence.
fn job_is_finished(status: &str) -> bool {
    matches!(status, "completed" | "failed" | JOB_STATUS_EXPIRED)
}

fn prune_jobs(jobs: &mut HashMap<String, TranscribeJob>, now: Instant) {
    for job in jobs.values_mut() {
        if job_is_finished(&job.status)
            && job.status != JOB_STATUS_EXPIRED
            && now.saturating_duration_since(job.created) >= JOB_TTL
        {
            job.status = JOB_STATUS_EXPIRED.to_string();
            job.transcript = None;
            job.error = None;
        }
    }

    let over = jobs.len().saturating_sub(MAX_JOBS);
    if over == 0 {
        return;
    }

    // Oldest first, finished jobs only.
    let mut evictable: Vec<(Instant, String)> = jobs
        .iter()
        .filter(|(_, job)| job_is_finished(&job.status))
        .map(|(id, job)| (job.created, id.clone()))
        .collect();
    evictable.sort_unstable_by_key(|(created, _)| *created);

    for (_, id) in evictable.into_iter().take(over) {
        jobs.remove(&id);
    }
}

async fn get_jobs() -> tokio::sync::MutexGuard<'static, Option<HashMap<String, TranscribeJob>>> {
    let mut guard = JOBS.lock().await;
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_health(State(state): State<Arc<ApiState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: state.app_version.clone(),
    })
}

async fn handle_get_state() -> Result<impl IntoResponse, AppError> {
    let state = crate::pipeline::get_pipeline_state();
    Ok(Json(state))
}

async fn handle_get_system() -> Result<impl IntoResponse, AppError> {
    let info = crate::platform::get_gpu_info()?;
    Ok(Json(info))
}

async fn handle_get_stats() -> Result<impl IntoResponse, AppError> {
    let stats = crate::database::transcription::get_transcription_stats_cmd()?;
    Ok(Json(stats))
}

async fn handle_get_prompts() -> impl IntoResponse {
    let prompts = crate::enhancement::prompts::get_all_prompts();
    Json(prompts)
}

async fn handle_get_settings() -> Result<impl IntoResponse, AppError> {
    let cfg = crate::config::get_config()?;
    Ok(Json(cfg))
}

/// Partially update settings. Accepts a JSON object with only the fields to change;
/// missing fields are preserved from the current config. camelCase keys are
/// canonicalised to snake_case before merging so both key styles are accepted.
async fn handle_patch_settings(
    Json(patch): Json<serde_json::Value>,
) -> Result<impl IntoResponse, AppError> {
    if !patch.is_object() {
        return Err(AppError::BadRequest(
            "PATCH /settings body must be a JSON object".to_string(),
        ));
    }
    let patch = crate::config::canonicalise_patch_keys(patch);
    let mut current = serde_json::to_value(crate::config::get_config()?)?;
    crate::config::merge_json(&mut current, &patch);
    let new_cfg: crate::config::Config =
        serde_json::from_value(current).map_err(|e| AppError::BadRequest(e.to_string()))?;
    crate::config::set_config(new_cfg)?;
    Ok(StatusCode::OK)
}

async fn handle_get_dictionary() -> Result<impl IntoResponse, AppError> {
    let entries = crate::dictionary::get_dictionary_entries()?;
    Ok(Json(entries))
}

/// Payload for POST /dictionary and PUT /dictionary/{index}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddEntryPayload {
    from: String,
    to: String,
    case_sensitive: bool,
}

async fn handle_add_dictionary(
    Json(payload): Json<AddEntryPayload>,
) -> Result<impl IntoResponse, AppError> {
    let entry = crate::dictionary::DictionaryEntry {
        from: payload.from,
        to: payload.to,
        case_sensitive: payload.case_sensitive,
    };
    crate::dictionary::add_dictionary_entry(entry)?;
    Ok(StatusCode::CREATED)
}

/// Reject an out-of-range dictionary index with 404 rather than letting the
/// underlying error map to a 500.
fn check_dictionary_index(index: usize) -> Result<(), AppError> {
    let count = crate::dictionary::get_dictionary_entries()?.len();
    if index >= count {
        return Err(AppError::NotFound(format!(
            "dictionary index {} out of range (have {} entries)",
            index, count
        )));
    }
    Ok(())
}

async fn handle_update_dictionary(
    Path(index): Path<usize>,
    Json(payload): Json<AddEntryPayload>,
) -> Result<impl IntoResponse, AppError> {
    check_dictionary_index(index)?;
    let entry = crate::dictionary::DictionaryEntry {
        from: payload.from,
        to: payload.to,
        case_sensitive: payload.case_sensitive,
    };
    crate::dictionary::update_dictionary_entry(index, entry)?;
    Ok(StatusCode::OK)
}

async fn handle_delete_dictionary(Path(index): Path<usize>) -> Result<impl IntoResponse, AppError> {
    check_dictionary_index(index)?;
    crate::dictionary::remove_dictionary_entry(index)?;
    Ok(StatusCode::OK)
}

/// Payload for POST /dictionary/import
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportPayload {
    json: String,
    merge: bool,
}

async fn handle_import_dictionary(
    Json(payload): Json<ImportPayload>,
) -> Result<impl IntoResponse, AppError> {
    let count = crate::dictionary::import_dictionary(payload.json, payload.merge)?;
    Ok(Json(serde_json::json!({ "imported": count })))
}

async fn handle_export_dictionary() -> Result<impl IntoResponse, AppError> {
    let body = crate::dictionary::export_dictionary()?;
    let value: serde_json::Value = serde_json::from_str(&body)?;
    Ok(Json(value))
}

async fn handle_get_transcription(Path(id): Path<String>) -> Result<impl IntoResponse, AppError> {
    match crate::database::transcription::get_transcription_by_id(id)? {
        Some(t) => Ok(Json(t).into_response()),
        None => Err(AppError::NotFound("transcription not found".to_string())),
    }
}

/// Payload for POST /transcribe
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranscribePayload {
    path: String,
}

/// Resolve a caller-supplied path, expanding a leading `~` to the home directory.
///
/// Supports `~` and `~/...`. `~user/...` is rejected rather than silently
/// treated as a literal directory name, so the error names the real cause
/// instead of surfacing as a confusing "file not found".
pub(crate) fn expand_user_path(path: &str) -> Result<std::path::PathBuf, String> {
    let Some(rest) = path.strip_prefix('~') else {
        return Ok(std::path::PathBuf::from(path));
    };

    // `~` on its own, or `~/...`. Anything else is `~user`, which would need a
    // passwd lookup to resolve and is not supported.
    let rest = match rest {
        "" => "",
        r if r.starts_with('/') => &r[1..],
        _ => {
            return Err(format!(
                "unsupported home-relative path: {}. Only `~` and `~/...` are \
                 supported; `~user` paths are not. Use an absolute path instead.",
                path
            ));
        }
    };

    let home = dirs::home_dir()
        .ok_or_else(|| "could not determine the home directory to expand `~`".to_string())?;

    Ok(if rest.is_empty() {
        home
    } else {
        home.join(rest)
    })
}

/// Submit a file for async transcription. Shared by the HTTP API and the MCP server.
///
/// Validates the path, registers a job, spawns the transcription off the executor
/// (and off the live recording pipeline), and returns the job id immediately.
pub(crate) async fn submit_transcribe_job(path: String) -> Result<String, String> {
    // Expand `~` before the existence check. The parameter has always been
    // documented as accepting a home-relative path, but the expansion was never
    // implemented, so `~/memo.m4a` failed as "file not found" and sent callers
    // hunting for a missing file rather than an unimplemented feature (#118).
    let resolved = expand_user_path(&path)?;

    if !resolved.is_file() {
        return Err(format!("file not found or not readable: {}", path));
    }

    let job_id = Uuid::new_v4().to_string();

    {
        let mut guard = get_jobs().await;
        let jobs = guard.as_mut().unwrap();
        prune_jobs(jobs, Instant::now());
        jobs.insert(
            job_id.clone(),
            TranscribeJob {
                status: "queued".to_string(),
                transcript: None,
                error: None,
                created: Instant::now(),
            },
        );
    }

    let job_id_clone = job_id.clone();
    tokio::spawn(async move {
        // Mark as processing.
        {
            let mut guard = get_jobs().await;
            if let Some(job) = guard.as_mut().unwrap().get_mut(&job_id_clone) {
                job.status = "processing".to_string();
            }
        }

        // transcribe_file is synchronous and CPU-bound.
        //
        // Background priority: this is batch work with no human waiting on it,
        // so it stands aside for live dictation instead of queueing ahead of it
        // (#118). Post-processing then puts the transcript through the same
        // filters, personal dictionary and canonical replacements the live path
        // applies, so a batch transcript is not the one place the user's own
        // vocabulary comes back mangled.
        let result = tokio::task::spawn_blocking(move || {
            let raw = crate::transcription::transcribe_file_with_priority(
                resolved.to_string_lossy().into_owned(),
                crate::transcription::Priority::Background,
            )?;
            let config = crate::pipeline::effective_pipeline_config()?;
            crate::pipeline::apply_text_post_processing(raw, &config)
                .map_err(crate::error::Error::from)
        })
        .await;

        let mut guard = get_jobs().await;
        if let Some(job) = guard.as_mut().unwrap().get_mut(&job_id_clone) {
            match result {
                Ok(Ok(transcript)) => {
                    job.status = "completed".to_string();
                    job.transcript = Some(transcript);
                }
                Ok(Err(e)) => {
                    job.status = "failed".to_string();
                    job.error = Some(e.to_string());
                }
                Err(e) => {
                    job.status = "failed".to_string();
                    job.error = Some(format!("task panicked: {}", e));
                }
            }
        }
    });

    Ok(job_id)
}

/// Ids of transcribe jobs that have not finished, oldest first.
///
/// Background file transcription runs on `spawn_blocking` and never touches the
/// live dictation pipeline's `PROCESSING_COUNT`, so `get_pipeline_state()` reports
/// `Idle` throughout. An agent that submitted a job and then asked what Thoth was
/// doing was told "nothing", which is how the MCP `get_state` tool came to ignore
/// its own background work.
pub(crate) async fn active_transcribe_jobs() -> Vec<String> {
    let guard = get_jobs().await;
    let mut active: Vec<(Instant, String)> = guard
        .as_ref()
        .unwrap()
        .iter()
        .filter(|(_, job)| !job_is_finished(&job.status))
        .map(|(id, job)| (job.created, id.clone()))
        .collect();
    active.sort_unstable_by_key(|(created, _)| *created);
    active.into_iter().map(|(_, id)| id).collect()
}

/// Look up a transcription job by id, returning its JSON representation. Shared with MCP.
///
/// A job past [`JOB_TTL`] reports `status: "expired"` rather than vanishing, so
/// the caller can tell "you polled too late" from "that id was never issued".
/// Expiry is computed here as well as in [`prune_jobs`], because the sweep only
/// runs on submission and a quiet process would otherwise keep serving a stale
/// transcript indefinitely.
pub(crate) async fn lookup_transcribe_job(id: &str) -> Option<serde_json::Value> {
    let mut guard = get_jobs().await;
    let job = guard.as_mut().unwrap().get_mut(id)?;

    if job_is_finished(&job.status)
        && job.status != JOB_STATUS_EXPIRED
        && Instant::now().saturating_duration_since(job.created) >= JOB_TTL
    {
        job.status = JOB_STATUS_EXPIRED.to_string();
        job.transcript = None;
        job.error = None;
    }

    serde_json::to_value(&*job).ok()
}

async fn handle_post_transcribe(
    Json(payload): Json<TranscribePayload>,
) -> Result<impl IntoResponse, AppError> {
    let job_id = submit_transcribe_job(payload.path)
        .await
        .map_err(AppError::BadRequest)?;
    Ok(Json(
        serde_json::json!({ "jobId": job_id, "status": "queued" }),
    ))
}

async fn handle_get_transcribe_job(Path(id): Path<String>) -> Result<impl IntoResponse, AppError> {
    match lookup_transcribe_job(&id).await {
        Some(job) => Ok(Json(job).into_response()),
        None => Err(AppError::NotFound(format!("job {} not found", id))),
    }
}

// ---------------------------------------------------------------------------
// Bearer-token auth layer
// ---------------------------------------------------------------------------

/// Build a [`ValidateRequestHeaderLayer`] that enforces bearer-token auth,
/// returning `{"error":"Unauthorized"}` with a 401 status on failure.
///
/// Uses `ValidateRequestHeaderLayer::custom` with a closure so the JSON error
/// body is preserved (the `accept` variant returns a bare status only).
// The Err variant is an axum::http::Response<Body> whose size is dictated by
// the tower ValidateRequestHeaderLayer API; boxing it would change the trait
// bound and break the layer type.
#[allow(clippy::result_large_err)]
fn bearer_auth_layer(
    token: String,
) -> ValidateRequestHeaderLayer<
    impl tower_http::validate_request::ValidateRequest<
        axum::body::Body,
        ResponseBody = axum::body::Body,
    > + Clone,
> {
    ValidateRequestHeaderLayer::custom(move |req: &mut axum::http::Request<axum::body::Body>| {
        // Constant-time compare: a plain `==` short-circuits on the first
        // differing byte, which leaks the token's length and prefix through
        // response-time variance. `ct_eq` folds over every byte regardless of
        // where (or whether) a mismatch occurs; a length mismatch is allowed
        // to short-circuit (length is not treated as sensitive).
        let ok = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .is_some_and(|t| t.as_bytes().ct_eq(token.as_bytes()).into());

        if ok {
            Ok(())
        } else {
            let body_bytes = serde_json::to_vec(&serde_json::json!({ "error": "Unauthorized" }))
                .unwrap_or_default();
            let body = axum::body::Body::from(body_bytes);
            let mut res = axum::http::Response::new(body);
            *res.status_mut() = StatusCode::UNAUTHORIZED;
            res.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("application/json"),
            );
            Err(res)
        }
    })
}

/// Build a [`ValidateRequestHeaderLayer`] that rejects any request whose
/// `Host` header does not name a loopback address, with a 400 and a JSON body
/// on failure.
///
/// Anti DNS-rebinding: a browser sets the `Host` header from the URL bar's
/// hostname, not the address it resolved to — so a malicious page served from
/// a public domain that resolves to `127.0.0.1` cannot forge a `Host` this
/// check accepts, even though the underlying TCP connection is genuinely
/// loopback. This mirrors rmcp's own default `allowed_hosts` behaviour on the
/// `/mcp` mount (see [`crate::mcp_server::build_service`]); the REST router
/// needs its own copy of the same check since rmcp only guards its own mount.
#[allow(clippy::result_large_err)]
fn host_validation_layer() -> ValidateRequestHeaderLayer<
    impl tower_http::validate_request::ValidateRequest<
        axum::body::Body,
        ResponseBody = axum::body::Body,
    > + Clone,
> {
    ValidateRequestHeaderLayer::custom(|req: &mut axum::http::Request<axum::body::Body>| {
        let ok = req
            .headers()
            .get(axum::http::header::HOST)
            .and_then(|v| v.to_str().ok())
            .is_some_and(is_loopback_host);

        if ok {
            Ok(())
        } else {
            let body_bytes =
                serde_json::to_vec(&serde_json::json!({ "error": "Invalid Host header" }))
                    .unwrap_or_default();
            let body = axum::body::Body::from(body_bytes);
            let mut res = axum::http::Response::new(body);
            *res.status_mut() = StatusCode::BAD_REQUEST;
            res.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("application/json"),
            );
            Err(res)
        }
    })
}

/// True when `host` (a raw `Host` header value, e.g. `127.0.0.1:3939` or
/// `[::1]`) names a loopback address. Any port is accepted — only the
/// hostname is checked, matching rmcp's own default `allowed_hosts` entries
/// (`localhost`, `127.0.0.1`, `::1`), which likewise carry no port.
fn is_loopback_host(host: &str) -> bool {
    let Ok(authority) = axum::http::uri::Authority::try_from(host) else {
        return false;
    };
    let hostname = authority
        .host()
        .trim_matches('[')
        .trim_matches(']')
        .to_ascii_lowercase();
    matches!(hostname.as_str(), "localhost" | "127.0.0.1" | "::1")
}

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

fn build_router(token: String, app_version: String, mcp_enabled: bool) -> Router {
    let state = Arc::new(ApiState { app_version });

    let auth = bearer_auth_layer(token.clone());

    let mut router = Router::new()
        .route("/health", get(handle_health))
        .route("/state", get(handle_get_state))
        .route("/system", get(handle_get_system))
        .route("/stats", get(handle_get_stats))
        .route("/prompts", get(handle_get_prompts))
        .route("/settings", get(handle_get_settings))
        .route("/settings", patch(handle_patch_settings))
        .route("/dictionary", get(handle_get_dictionary))
        .route("/dictionary", post(handle_add_dictionary))
        .route("/dictionary/import", post(handle_import_dictionary))
        .route("/dictionary/export", get(handle_export_dictionary))
        .route("/dictionary/{index}", put(handle_update_dictionary))
        .route("/dictionary/{index}", delete(handle_delete_dictionary))
        .route("/transcriptions/{id}", get(handle_get_transcription))
        .route("/transcribe", post(handle_post_transcribe))
        .route("/transcribe/{id}", get(handle_get_transcribe_job))
        .layer(auth)
        .layer(host_validation_layer())
        .with_state(state);

    // Mount the bundled MCP server at /mcp when enabled, behind the same bearer auth.
    if mcp_enabled {
        let mcp_service = crate::mcp_server::build_service();
        let mcp_router = Router::new()
            .nest_service("/mcp", mcp_service)
            .layer(bearer_auth_layer(token));
        router = router.merge(mcp_router);
        tracing::info!("MCP server mounted at /mcp");
    }

    router
}

// ---------------------------------------------------------------------------
// Start / stop
// ---------------------------------------------------------------------------

/// Start the control API server.
///
/// If a server is already running it is stopped first.
/// Binds 127.0.0.1:{port} only.
pub async fn start(port: u16, token: String, mcp_enabled: bool) -> Result<(), String> {
    // Stop any existing server before starting a new one.
    stop().await;

    let app_version = env!("CARGO_PKG_VERSION").to_string();
    let router = build_router(token, app_version, mcp_enabled);

    let addr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Control API: failed to bind {}:{}: {}", addr.ip(), port, e);
            telemetry::report_error_with_cause("control_api_failed", &e);
            tracing::error!(target: TELEMETRY_TARGET, error = %e, port, "control_api_failed");
            return Err(format!(
                "Could not start the server on port {port}: {e}. \
                 The port may already be in use — try a different port."
            ));
        }
    };

    tracing::info!("Control API listening on {}", addr);

    let server_fut = axum::serve(listener, router);
    let join_handle = tokio::spawn(async move {
        if let Err(e) = server_fut.await {
            tracing::error!("Control API server error: {}", e);
            telemetry::report_error_with_cause("control_api_failed", &e);
            tracing::error!(target: TELEMETRY_TARGET, error = %e, "control_api_failed");
        }
    });

    let mut guard = SERVER.lock().await;
    *guard = Some(ServerHandle {
        abort: join_handle.abort_handle(),
    });
    Ok(())
}

/// Stop the running control API server, if any.
pub async fn stop() {
    let mut guard = SERVER.lock().await;
    if let Some(handle) = guard.take() {
        handle.abort.abort();
        tracing::info!("Control API server stopped");
    }
}

/// Returns true if the server task is currently running (not aborted).
pub async fn is_running() -> bool {
    let guard = SERVER.lock().await;
    guard.is_some()
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Status of the integrations (reported to the frontend).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationsStatus {
    pub api_enabled: bool,
    pub api_running: bool,
    pub api_port: u16,
    pub mcp_enabled: bool,
    pub has_token: bool,
}

/// Return the current integrations status for the frontend settings panel.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn get_integrations_status() -> Result<IntegrationsStatus, Error> {
    let cfg = crate::config::get_config()?;
    let running = is_running().await;
    Ok(IntegrationsStatus {
        api_enabled: cfg.integrations.api_enabled,
        api_running: running,
        api_port: cfg.integrations.api_port,
        mcp_enabled: cfg.integrations.mcp_enabled,
        has_token: token_store::read_token().is_some(),
    })
}

/// Enable or disable the Local Control API.
///
/// When enabling: generates a token if none exists, then starts the server.
/// When disabling: stops the server and persists the updated flag.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn set_api_enabled(app: tauri::AppHandle, enabled: bool) -> Result<(), Error> {
    let _ = app; // AppHandle reserved for future event emission
    let mut cfg = crate::config::get_config()?;
    cfg.integrations.api_enabled = enabled;
    let port = cfg.integrations.api_port;
    let mcp = cfg.integrations.mcp_enabled;
    crate::config::set_config(cfg)?;

    if enabled {
        start(port, token_store::get_or_create_token(), mcp).await?;
    } else {
        stop().await;
    }
    Ok(())
}

/// Enable or disable the bundled MCP server.
///
/// The MCP server mounts at `/mcp` on the same loopback HTTP server as the
/// Control API. Enabling MCP also enables and starts the Control API if it
/// isn't already running (the MCP route can't exist without the host server).
/// The route change takes effect immediately; no app restart is required.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn set_mcp_enabled(enabled: bool) -> Result<(), Error> {
    let mut cfg = crate::config::get_config()?;
    cfg.integrations.mcp_enabled = enabled;

    // Enabling MCP implies the Control API must be on to host the /mcp route.
    if enabled {
        cfg.integrations.api_enabled = true;
    }

    let port = cfg.integrations.api_port;
    let api_enabled = cfg.integrations.api_enabled;
    crate::config::set_config(cfg)?;

    // Restart (or start) the server so the /mcp route appears/disappears live.
    // Awaiting start() means the bind has completed before we return, so the
    // status the UI reads immediately afterwards is accurate.
    if api_enabled {
        start(port, token_store::get_or_create_token(), enabled).await?;
    }
    Ok(())
}

/// Return the current API token for display/copy in the settings panel.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn get_api_token() -> Result<Option<String>, Error> {
    Ok(Some(token_store::get_or_create_token()))
}

/// Generate a new token, persist it, and restart the server if running.
///
/// Returns the new token so the frontend can display it immediately.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn rotate_api_token(app: tauri::AppHandle) -> Result<String, Error> {
    let _ = app;
    let new_token = token_store::rotate();
    let cfg = crate::config::get_config()?;
    let was_running = is_running().await;
    let port = cfg.integrations.api_port;
    let mcp = cfg.integrations.mcp_enabled;

    if was_running {
        start(port, new_token.clone(), mcp).await?;
    }
    Ok(new_token)
}

/// Change the API port. Restarts the server on the new port if it was running.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub async fn set_api_port(app: tauri::AppHandle, port: u16) -> Result<(), Error> {
    let _ = app;
    let mut cfg = crate::config::get_config()?;
    cfg.integrations.api_port = port;
    let was_running = is_running().await;
    let mcp = cfg.integrations.mcp_enabled;
    crate::config::set_config(cfg)?;

    if was_running {
        start(port, token_store::get_or_create_token(), mcp).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    /// Build a job of the given age and status, for the registry tests.
    fn aged_job(status: &str, age: Duration, transcript: Option<&str>) -> TranscribeJob {
        TranscribeJob {
            status: status.to_string(),
            transcript: transcript.map(str::to_string),
            error: None,
            created: Instant::now()
                .checked_sub(age)
                .expect("test clock underflow"),
        }
    }

    /// A transcript must not sit in memory forever. Thoth is a menu-bar app that
    /// stays resident for days, and before #172 the registry had no eviction of
    /// any kind — no TTL, no cap, no removal on read.
    #[test]
    fn prune_expires_transcripts_past_the_ttl() {
        let mut jobs = HashMap::new();
        jobs.insert(
            "old".to_string(),
            aged_job("completed", JOB_TTL + Duration::from_secs(1), Some("hello")),
        );
        jobs.insert(
            "fresh".to_string(),
            aged_job("completed", Duration::from_secs(1), Some("hello")),
        );

        prune_jobs(&mut jobs, Instant::now());

        let old = &jobs["old"];
        assert_eq!(old.status, JOB_STATUS_EXPIRED);
        assert!(old.transcript.is_none(), "expired job kept its transcript");

        let fresh = &jobs["fresh"];
        assert_eq!(fresh.status, "completed", "a fresh job was expired");
        assert_eq!(fresh.transcript.as_deref(), Some("hello"));
    }

    /// The cap is what makes the registry genuinely bounded: expiry alone leaves a
    /// tombstone per job, which still grows without limit over months of uptime.
    #[test]
    fn prune_caps_the_registry_oldest_first() {
        let mut jobs = HashMap::new();
        for i in 0..(MAX_JOBS + 50) {
            // Older index = older job, so the first 50 are the eviction victims.
            let age = Duration::from_secs((MAX_JOBS + 50 - i) as u64);
            jobs.insert(format!("job-{i}"), aged_job("completed", age, Some("x")));
        }

        prune_jobs(&mut jobs, Instant::now());

        assert_eq!(jobs.len(), MAX_JOBS, "registry was not capped");
        assert!(!jobs.contains_key("job-0"), "oldest job survived the cap");
        assert!(
            jobs.contains_key(&format!("job-{}", MAX_JOBS + 49)),
            "newest job was evicted"
        );
    }

    /// Transcription is deliberately starvable: background priority stands aside
    /// for live dictation indefinitely, so a job can still be running an hour
    /// after submission. Expiring it would report `expired` with a null
    /// transcript for work that is about to succeed — and would then make it
    /// evictable, at which point its completion handler finds nothing and the
    /// real transcript is discarded in silence.
    #[test]
    fn prune_never_expires_an_in_flight_job() {
        let past_ttl = JOB_TTL + Duration::from_secs(60);
        let mut jobs = HashMap::new();
        jobs.insert(
            "running".to_string(),
            aged_job("processing", past_ttl, None),
        );
        jobs.insert("waiting".to_string(), aged_job("queued", past_ttl, None));
        jobs.insert(
            "done".to_string(),
            aged_job("completed", past_ttl, Some("x")),
        );

        prune_jobs(&mut jobs, Instant::now());

        assert_eq!(
            jobs["running"].status, "processing",
            "a running job expired"
        );
        assert_eq!(jobs["waiting"].status, "queued", "a queued job expired");
        assert_eq!(
            jobs["done"].status, JOB_STATUS_EXPIRED,
            "a finished job past the TTL should still expire"
        );
    }

    /// The same guard has to hold on the read path, which expires independently
    /// of the sweep so a quiet process cannot serve a stale transcript forever.
    #[tokio::test]
    async fn lookup_never_expires_an_in_flight_job() {
        let id = format!("in-flight-{}", Uuid::new_v4());
        {
            let mut guard = get_jobs().await;
            guard.as_mut().unwrap().insert(
                id.clone(),
                aged_job("processing", JOB_TTL + Duration::from_secs(60), None),
            );
        }

        let job = lookup_transcribe_job(&id).await.expect("job vanished");
        assert_eq!(
            job["status"], "processing",
            "polling a still-running job past the TTL reported it expired"
        );

        get_jobs().await.as_mut().unwrap().remove(&id);
    }

    /// A job still queued or processing is what `get_state` must see, and it is
    /// the same predicate the TTL and the cap step over.
    #[tokio::test]
    async fn active_jobs_lists_only_unfinished_work() {
        let running = format!("active-{}", Uuid::new_v4());
        let done = format!("done-{}", Uuid::new_v4());
        {
            let mut guard = get_jobs().await;
            let jobs = guard.as_mut().unwrap();
            jobs.insert(
                running.clone(),
                aged_job("processing", Duration::from_secs(1), None),
            );
            jobs.insert(
                done.clone(),
                aged_job("completed", Duration::from_secs(1), Some("x")),
            );
        }

        let active = active_transcribe_jobs().await;
        assert!(active.contains(&running), "a processing job was not listed");
        assert!(
            !active.contains(&done),
            "a completed job was listed as active"
        );

        let mut guard = get_jobs().await;
        let jobs = guard.as_mut().unwrap();
        jobs.remove(&running);
        jobs.remove(&done);
    }

    /// An in-flight job's completion handler writes back by id. Evicting it would
    /// throw the transcript away with no error raised anywhere, so the cap must
    /// step over anything not yet finished.
    #[test]
    fn prune_never_evicts_an_in_flight_job() {
        let mut jobs = HashMap::new();
        jobs.insert(
            "running".to_string(),
            aged_job(
                "processing",
                Duration::from_secs(MAX_JOBS as u64 + 100),
                None,
            ),
        );
        jobs.insert(
            "waiting".to_string(),
            aged_job("queued", Duration::from_secs(MAX_JOBS as u64 + 99), None),
        );
        for i in 0..(MAX_JOBS + 50) {
            let age = Duration::from_secs((MAX_JOBS + 50 - i) as u64);
            jobs.insert(format!("job-{i}"), aged_job("completed", age, Some("x")));
        }

        prune_jobs(&mut jobs, Instant::now());

        assert!(jobs.contains_key("running"), "a processing job was evicted");
        assert!(jobs.contains_key("waiting"), "a queued job was evicted");
    }

    /// The whole point of expiring rather than deleting: a caller polling a
    /// long-finished job must learn it polled too late, not that its id never
    /// existed. Both the HTTP handler and the MCP `transcribe_status` tool read
    /// this one function, so the distinction reaches both surfaces.
    #[tokio::test]
    async fn lookup_reports_expiry_distinctly_from_an_unknown_id() {
        let id = format!("expiry-test-{}", Uuid::new_v4());
        {
            let mut guard = get_jobs().await;
            guard.as_mut().unwrap().insert(
                id.clone(),
                aged_job("completed", JOB_TTL + Duration::from_secs(1), Some("hi")),
            );
        }

        let expired = lookup_transcribe_job(&id)
            .await
            .expect("an expired job must still resolve, or it is just a 404");
        assert_eq!(expired["status"], JOB_STATUS_EXPIRED);
        assert!(
            expired["transcript"].is_null(),
            "expired job still returned a transcript: {expired}"
        );

        assert!(
            lookup_transcribe_job(&Uuid::new_v4().to_string())
                .await
                .is_none(),
            "an id that was never issued must not resolve"
        );

        get_jobs().await.as_mut().unwrap().remove(&id);
    }
    use super::*;

    #[test]
    fn token_has_canonical_shape() {
        let t = generate_token();
        assert!(t.starts_with("sk-thoth-"), "prefix: {}", t);
        assert_eq!(t.len(), TOKEN_PREFIX.len() + 40, "length: {}", t);
        let secret = &t[TOKEN_PREFIX.len()..];
        assert!(
            secret.bytes().all(|b| BASE62.contains(&b)),
            "secret must be base62: {}",
            secret
        );
    }

    #[test]
    fn tokens_are_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
    }

    // Path handling for `transcribe_file`. The `path` parameter has always
    // documented `~` support; these pin that the documentation and the code
    // now agree, in either direction (#118).

    #[test]
    fn tilde_slash_path_expands_to_the_home_directory() {
        let home = dirs::home_dir().expect("home directory required for this test");

        let expanded = expand_user_path("~/memo.m4a").expect("`~/...` must expand");

        assert_eq!(expanded, home.join("memo.m4a"));
        assert!(
            expanded.is_absolute(),
            "expansion must yield an absolute path: {}",
            expanded.display()
        );
    }

    #[test]
    fn bare_tilde_expands_to_the_home_directory() {
        let home = dirs::home_dir().expect("home directory required for this test");
        assert_eq!(expand_user_path("~").expect("`~` must expand"), home);
    }

    #[test]
    fn absolute_path_is_returned_unchanged() {
        let path = "/var/audio/memo.wav";
        assert_eq!(
            expand_user_path(path).expect("absolute paths must pass through"),
            std::path::PathBuf::from(path)
        );
    }

    #[test]
    fn relative_path_is_returned_unchanged() {
        // Not expanded, not rejected: resolved against the process working
        // directory by the existence check, as any other relative path is.
        let path = "audio/memo.wav";
        assert_eq!(
            expand_user_path(path).expect("relative paths must pass through"),
            std::path::PathBuf::from(path)
        );
    }

    #[test]
    fn tilde_user_path_is_rejected_by_naming_the_real_cause() {
        let err = expand_user_path("~someone/memo.wav")
            .expect_err("`~user` paths must be rejected, not silently mangled");

        // The point of the rejection is that it does NOT masquerade as a
        // missing file, which is what sent the original reporter looking in
        // the wrong place.
        assert!(
            !err.contains("file not found"),
            "error must not look like a missing file: {err}"
        );
        assert!(
            err.contains("~user"),
            "error must name the unsupported form: {err}"
        );
    }

    #[tokio::test]
    async fn missing_file_still_reports_not_found() {
        let err = submit_transcribe_job("/nonexistent/thoth/memo.wav".to_string())
            .await
            .expect_err("a missing file must not be accepted as a job");

        assert!(
            err.contains("file not found or not readable"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn missing_file_under_tilde_reports_not_found_after_expansion() {
        let err = submit_transcribe_job("~/nonexistent-thoth-test-file.wav".to_string())
            .await
            .expect_err("a missing file must not be accepted as a job");

        // Expansion succeeded; the failure is a genuine missing file, so the
        // not-found error is the honest one here.
        assert!(
            err.contains("file not found or not readable"),
            "unexpected error: {err}"
        );
    }
}
