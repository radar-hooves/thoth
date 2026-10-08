//! Bundled MCP server — exposes Thoth's control surface to MCP clients.
//!
//! Implemented with rmcp (the official Rust MCP SDK). Mounts as a tower service
//! on the same loopback axum router as the Control API, behind the same bearer-token
//! auth. Tools call the same shared core functions the GUI and HTTP API use; no
//! business logic lives here (transport only). Opt-in via `integrations.mcpEnabled`.
//!
//! Tool surface (task-centric, per the house MCP design principles):
//! - `dictionary` (dispatcher: list/add/update/delete/import/export)
//! - `canonical`  (dispatcher: list/add/update/remove)
//! - `setting`    (dispatcher: get/update)
//! - `transcription` (dispatcher: list/get/stats)
//! - `transcribe_file` / `transcribe_status` (async file transcription)
//! - `recording` (start/stop/toggle, mirroring the global hotkey)
//! - `get_state`, `get_system`, `list_prompts`

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};

/// The MCP server state. Cloned per session by the transport's service factory.
#[derive(Clone)]
pub struct ThothMcp {
    // Read by the `#[tool_handler]` macro-generated `ServerHandler` impl.
    #[allow(dead_code)]
    tool_router: ToolRouter<ThothMcp>,
}

impl Default for ThothMcp {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tool parameter types
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DictionaryParams {
    /// The operation: `list`, `add`, `update`, `delete`, `import`, or `export`.
    pub action: String,
    /// For `add`: the text to find. For `update`/`delete`: identifies WHICH entry to
    /// act on, by its own `from` text (case-insensitive) — the safe way in, since it
    /// cannot silently hit the wrong entry. Refuses if it matches more than one.
    #[serde(default)]
    pub from: Option<String>,
    /// For `add`/`update`: the replacement text.
    #[serde(default)]
    pub to: Option<String>,
    /// For `add`: whether the match is case-sensitive (default false). For `update`:
    /// omit to keep the entry's current setting.
    #[serde(default)]
    pub case_sensitive: Option<bool>,
    /// For `update`/`delete`: the zero-based index of the entry, as reported in each
    /// row of `list`. Prefer `from`: an index counted out by hand silently edits or
    /// deletes a different, working entry when it is wrong.
    #[serde(default)]
    pub index: Option<usize>,
    /// For `import`: a JSON string of entries — exactly what `export` returns,
    /// e.g. `{"entries":[{"from":"x","to":"y","caseSensitive":false}]}`. A bare
    /// array of those entry objects (`[{"from":"x","to":"y","caseSensitive":false}]`)
    /// also works.
    #[serde(default)]
    pub json: Option<String>,
    /// For `import`: merge with existing entries (true) or replace (false). Default true.
    #[serde(default)]
    pub merge: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CanonicalParams {
    /// The operation: `list`, `add`, `update`, `remove`, or `suggest`.
    pub action: String,
    /// For `add`/`update`: the canonical term string (e.g. "portcullis", "LiteLLM").
    #[serde(default)]
    pub term: Option<String>,
    /// For `add`/`update`: explicit spelling aliases (case-insensitive exact
    /// matches). `update` replaces the whole term, so omitting this CLEARS the
    /// term's aliases — send back everything `list` showed that you want kept.
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
    /// For `add`/`update`: matching policy — `aliasOnly` (default), `phonetic`,
    /// or `conservative`. Omitting it on `update` resets the term to `aliasOnly`.
    #[serde(default)]
    pub policy: Option<String>,
    /// For `update`/`remove`: the zero-based index of the term (from `list`).
    #[serde(default)]
    pub index: Option<usize>,
    /// For `suggest`: how many recent transcriptions to scan. Default 2000.
    #[serde(default)]
    pub history_limit: Option<i64>,
    /// For `suggest`: how often a spelling must occur to be reported. Default 2.
    #[serde(default)]
    pub min_occurrences: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SettingParams {
    /// The operation: `get` (read all settings) or `update`.
    pub action: String,
    /// For `update`: a JSON object of settings to change, merged onto the current
    /// config (e.g. `{"enhancement":{"enabled":true}}`). Missing fields keep their values.
    #[serde(default)]
    pub patch: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TranscriptionParams {
    /// The operation: `list` (recent history), `get` (one by id), or `stats` (quality).
    pub action: String,
    /// For `get`: the transcription id.
    #[serde(default)]
    pub id: Option<String>,
    /// For `list`: keep only records whose text contains this (case-insensitive),
    /// as the history search in the app does.
    #[serde(default)]
    pub query: Option<String>,
    /// For `list`: how many records, newest first. Default 100.
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TranscribeFileParams {
    /// Absolute or `~`-relative path to a local audio file (WAV, MP3, M4A, OGG, FLAC).
    /// `~` and `~/...` are expanded; `~user/...` is not supported.
    pub path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TranscribeStatusParams {
    /// The job id returned by `transcribe_file`.
    pub job_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RecordingParams {
    /// The action: `start`, `stop`, or `toggle` (toggle mirrors the global hotkey).
    pub action: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a successful text result from a serialisable value (compact JSON, no
/// whitespace) to keep MCP responses token-cheap, matching FastMCP servers.
fn json_result<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let text =
        serde_json::to_string(value).map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Map a core-call error message into an MCP tool error on failure.
fn core_err(msg: String) -> McpError {
    McpError::internal_error(msg, None)
}

/// Compact acknowledgement for a mutating dictionary/canonical action. Confirms
/// success and reports the affected `index` and the resulting collection size,
/// WITHOUT echoing the whole list back. Returning the full ~150-entry list on
/// every add/update/delete was pure token waste; a caller that wants the list
/// calls the `list` action.
fn mutation_ack(action: &str, index: usize, count: usize) -> Result<CallToolResult, McpError> {
    json_result(&serde_json::json!({
        "ok": true,
        "action": action,
        "index": index,
        "count": count,
    }))
}

/// Parse an optional policy string into a `SnapPolicy`.  `None` defaults to `AliasOnly`.
fn parse_snap_policy(policy: Option<&str>) -> Result<crate::canonical::SnapPolicy, McpError> {
    match policy {
        None | Some("aliasOnly") => Ok(crate::canonical::SnapPolicy::AliasOnly),
        Some("phonetic") => Ok(crate::canonical::SnapPolicy::Phonetic),
        Some("conservative") => Ok(crate::canonical::SnapPolicy::Conservative),
        Some(other) => Err(core_err(format!(
            "unknown policy '{}'; must be aliasOnly | phonetic | conservative",
            other
        ))),
    }
}

#[tool_router]
impl ThothMcp {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Manage Thoth's personal dictionary (find/replace entries applied to transcriptions). Use this to view or change spelling corrections and word replacements. Action: list | add | update | delete | import | export. add requires from + to (+ optional caseSensitive). update/delete identify the entry EITHER by from (its own text — preferred, refuses if it is not unique) OR by index; update also requires to. import requires json — exactly what `export` returns, e.g. {\"entries\":[{\"from\":\"x\",\"to\":\"y\",\"caseSensitive\":false}]}; a bare array of those entry objects also works (+ optional merge, default true: dedupe by `from`; false replaces the whole dictionary). Returns: the entry list, each row carrying its index (list), a compact ack {ok, action, index, count} (add/update/delete), a count (import), or a JSON string (export)."
    )]
    async fn dictionary(
        &self,
        Parameters(p): Parameters<DictionaryParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "list" => {
                let entries = crate::dictionary::get_dictionary_entries()
                    .map_err(|e| core_err(e.to_string()))?;
                // Each row carries its own index: the index-based form of
                // update/delete is unusable if the caller has to count a
                // three-hundred-entry response by eye to find one.
                let rows: Vec<_> = entries
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        serde_json::json!({
                            "index": i,
                            "from": e.from,
                            "to": e.to,
                            "caseSensitive": e.case_sensitive,
                        })
                    })
                    .collect();
                json_result(&rows)
            }
            "add" => {
                let entry = crate::dictionary::DictionaryEntry {
                    from: p
                        .from
                        .ok_or_else(|| core_err("`from` required for add".into()))?,
                    to: p
                        .to
                        .ok_or_else(|| core_err("`to` required for add".into()))?,
                    case_sensitive: p.case_sensitive.unwrap_or(false),
                };
                crate::dictionary::add_dictionary_entry(entry)
                    .map_err(|e| core_err(e.to_string()))?;
                let count = crate::dictionary::get_dictionary_entries()
                    .map_err(|e| core_err(e.to_string()))?
                    .len();
                // add appends, so the new entry sits at the last index.
                mutation_ack("add", count.saturating_sub(1), count)
            }
            // `update` and `delete` both take EITHER an explicit `index` (the
            // original form, unchanged) or the entry's own `from` text. The
            // by-`from` path resolves and mutates under one write lock in the
            // dictionary core, and refuses rather than guessing when `from`
            // matches zero or several entries.
            "update" => {
                let to =
                    p.to.ok_or_else(|| core_err("`to` required for update".into()))?;
                let index = match p.index {
                    Some(index) => {
                        let entry = crate::dictionary::DictionaryEntry {
                            from: p.from.ok_or_else(|| {
                                core_err("`from` required when updating by `index`".into())
                            })?,
                            to,
                            case_sensitive: p.case_sensitive.unwrap_or(false),
                        };
                        crate::dictionary::update_dictionary_entry(index, entry)
                            .map_err(|e| core_err(e.to_string()))?;
                        index
                    }
                    None => {
                        let key = p.from.ok_or_else(|| {
                            core_err("update needs `from` (the entry's own text) or `index`".into())
                        })?;
                        crate::dictionary::update_dictionary_entry_by_from(
                            &key,
                            to,
                            p.case_sensitive,
                        )
                        .map_err(|e| core_err(e.to_string()))?
                    }
                };
                let count = crate::dictionary::get_dictionary_entries()
                    .map_err(|e| core_err(e.to_string()))?
                    .len();
                mutation_ack("update", index, count)
            }
            "delete" => {
                let index = match p.index {
                    Some(index) => {
                        crate::dictionary::remove_dictionary_entry(index)
                            .map_err(|e| core_err(e.to_string()))?;
                        index
                    }
                    None => {
                        let key = p.from.ok_or_else(|| {
                            core_err("delete needs `from` (the entry's own text) or `index`".into())
                        })?;
                        crate::dictionary::remove_dictionary_entry_by_from(&key)
                            .map_err(|e| core_err(e.to_string()))?
                    }
                };
                let count = crate::dictionary::get_dictionary_entries()
                    .map_err(|e| core_err(e.to_string()))?
                    .len();
                mutation_ack("delete", index, count)
            }
            "import" => {
                let json = p
                    .json
                    .ok_or_else(|| core_err("`json` required for import".into()))?;
                let count = crate::dictionary::import_dictionary(json, p.merge.unwrap_or(true))
                    .map_err(|e| core_err(e.to_string()))?;
                json_result(&serde_json::json!({ "imported": count }))
            }
            "export" => {
                let json =
                    crate::dictionary::export_dictionary().map_err(|e| core_err(e.to_string()))?;
                Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
            }
            other => Err(core_err(format!(
                "unknown action '{}'; must be list | add | update | delete | import | export",
                other
            ))),
        }
    }

    #[tool(
        description = "Manage Thoth's canonical-term registry (phonetic/fuzzy snapping of acoustic variants to a registered spelling). Register a term ONCE and all acoustic/spelling variants auto-snap to it. Action: list | add | update | remove | suggest. add/update require term; update/remove also require index (from `list`). NOTE: update REPLACES the whole term — omit `aliases` and the term keeps none, omit `policy` and it resets to aliasOnly; read the current term with `list` first and send back everything you want kept. policy: aliasOnly (default, exact aliases only), phonetic (AND gate: Double-Metaphone key match AND edit-distance >= 0.55), conservative (same AND gate, higher 0.85 threshold for terms that collide with common words). `suggest` reads the transcription history and reports spellings that look like a registered term but were NOT snapped to it — the mis-hearings still escaping, ranked by how often they occur, each with the term's own frequency and a line of context. It is ADVISORY and changes nothing: judge each row (an ordinary English word will read as one in its context) and register the real ones with `update`. Returns: the term list (list), a compact ack {ok, action, index, count} (add/update/remove), or the candidate list (suggest)."
    )]
    async fn canonical(
        &self,
        Parameters(p): Parameters<CanonicalParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "list" => {
                let terms =
                    crate::canonical::get_canonical_terms().map_err(|e| core_err(e.to_string()))?;
                json_result(&terms)
            }
            "add" => {
                let term_str = p
                    .term
                    .ok_or_else(|| core_err("`term` required for add".into()))?;
                let policy = parse_snap_policy(p.policy.as_deref())?;
                let ct = crate::canonical::CanonicalTerm {
                    term: term_str,
                    aliases: p.aliases.unwrap_or_default(),
                    policy,
                    max_words: 3,
                    threshold: None,
                };
                crate::canonical::add_canonical_term(ct).map_err(|e| core_err(e.to_string()))?;
                let count = crate::canonical::get_canonical_terms()
                    .map_err(|e| core_err(e.to_string()))?
                    .len();
                // add appends, so the new term sits at the last index.
                mutation_ack("add", count.saturating_sub(1), count)
            }
            "update" => {
                let index = p
                    .index
                    .ok_or_else(|| core_err("`index` required for update".into()))?;
                let term_str = p
                    .term
                    .ok_or_else(|| core_err("`term` required for update".into()))?;
                let policy = parse_snap_policy(p.policy.as_deref())?;
                let ct = crate::canonical::CanonicalTerm {
                    term: term_str,
                    aliases: p.aliases.unwrap_or_default(),
                    policy,
                    max_words: 3,
                    threshold: None,
                };
                crate::canonical::update_canonical_term(index, ct)
                    .map_err(|e| core_err(e.to_string()))?;
                let count = crate::canonical::get_canonical_terms()
                    .map_err(|e| core_err(e.to_string()))?
                    .len();
                mutation_ack("update", index, count)
            }
            "remove" => {
                let index = p
                    .index
                    .ok_or_else(|| core_err("`index` required for remove".into()))?;
                crate::canonical::remove_canonical_term(index)
                    .map_err(|e| core_err(e.to_string()))?;
                let count = crate::canonical::get_canonical_terms()
                    .map_err(|e| core_err(e.to_string()))?
                    .len();
                mutation_ack("remove", index, count)
            }
            "suggest" => {
                // Clamped: SQLite reads a negative LIMIT as "no limit", so an
                // unchecked -1 would pull the whole history into memory.
                let limit = p.history_limit.unwrap_or(2000).clamp(1, 100_000);
                let suggestions = crate::canonical::suggest_aliases_from_history(
                    limit,
                    p.min_occurrences.unwrap_or(2),
                )
                .map_err(|e| core_err(e.to_string()))?;
                json_result(&suggestions)
            }
            other => Err(core_err(format!(
                "unknown action '{}'; must be list | add | update | remove | suggest",
                other
            ))),
        }
    }

    #[tool(
        description = "Read or change Thoth's settings (AI enhancement on/off, backend, prompt; output filters; Australian spelling; selected model; sounds; word list sync with a WebDAV file). Action: get | update. update requires patch — a JSON object of fields to change, merged onto the current config. Returns: the full settings object."
    )]
    async fn setting(
        &self,
        Parameters(p): Parameters<SettingParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "get" => {
                let cfg = crate::config::get_config().map_err(|e| core_err(e.to_string()))?;
                json_result(&cfg)
            }
            "update" => {
                let patch = p
                    .patch
                    .ok_or_else(|| core_err("`patch` required for update".into()))?;
                // Canonicalise camelCase keys to snake_case before merging so that
                // `{"apiPort": 8765}` and `{"api_port": 8765}` both work. Without this step, a camelCase key is added alongside the
                // existing snake_case key and serde errors on "duplicate field".
                let patch_val: serde_json::Value = serde_json::from_str(&patch)
                    .map_err(|e| core_err(format!("invalid patch JSON: {}", e)))?;
                if !patch_val.is_object() {
                    return Err(core_err(
                        "`patch` must be a JSON object, not a scalar or array".into(),
                    ));
                }
                let patch_val = crate::config::canonicalise_patch_keys(patch_val);
                let mut current = serde_json::to_value(
                    crate::config::get_config().map_err(|e| core_err(e.to_string()))?,
                )
                .map_err(|e| core_err(e.to_string()))?;
                crate::config::merge_json(&mut current, &patch_val);
                let new_cfg: crate::config::Config = serde_json::from_value(current)
                    .map_err(|e| core_err(format!("merged config invalid: {}", e)))?;
                crate::config::set_config(new_cfg).map_err(|e| core_err(e.to_string()))?;
                let cfg = crate::config::get_config().map_err(|e| core_err(e.to_string()))?;
                json_result(&cfg)
            }
            other => Err(core_err(format!(
                "unknown action '{}'; must be get | update",
                other
            ))),
        }
    }

    #[tool(
        description = "Read transcription history and quality. Action: list (most recent records, newest first: `limit` of them, default 100; `query` keeps only those whose text contains it), get (one by id), stats (counts, average duration, per-model throughput). get requires id. Returns: records (list), a record (get), or summary statistics (stats)."
    )]
    async fn transcription(
        &self,
        Parameters(p): Parameters<TranscriptionParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action.as_str() {
            "stats" => {
                let stats = crate::database::transcription::get_transcription_stats_cmd()
                    .map_err(|e| core_err(e.to_string()))?;
                json_result(&stats)
            }
            "get" => {
                let id =
                    p.id.ok_or_else(|| core_err("`id` required for get".into()))?;
                let record = crate::database::transcription::get_transcription_by_id(id.clone())
                    .map_err(|e| core_err(e.to_string()))?;
                match record {
                    Some(r) => json_result(&r),
                    None => Err(core_err(format!("transcription {} not found", id))),
                }
            }
            "list" => {
                let limit = Some(p.limit.unwrap_or(100));
                let records = match p.query.as_deref() {
                    Some(q) => crate::database::transcription::search_transcriptions(q, limit),
                    None => crate::database::transcription::list_transcriptions(limit, None),
                }
                .map_err(|e| core_err(e.to_string()))?;
                json_result(&records)
            }
            other => Err(core_err(format!(
                "unknown action '{}'; must be list | get | stats",
                other
            ))),
        }
    }

    #[tool(
        description = "Transcribe a local audio file through Thoth (WAV, MP3, M4A, OGG, FLAC). Runs as a background job at lower priority than live recording: file jobs run one at a time and yield the transcription model to live dictation, so submitting a batch will not take the microphone away from someone dictating. The transcript has the same output filters, personal dictionary and canonical replacements applied as live dictation; it does NOT get AI enhancement. Returns a jobId; poll `transcribe_status` for the transcript."
    )]
    async fn transcribe_file(
        &self,
        Parameters(p): Parameters<TranscribeFileParams>,
    ) -> Result<CallToolResult, McpError> {
        let job_id = crate::control_api::submit_transcribe_job(p.path)
            .await
            .map_err(|e| core_err(e.to_string()))?;
        json_result(&serde_json::json!({ "jobId": job_id, "status": "queued" }))
    }

    #[tool(
        description = "Check the status of a `transcribe_file` job. Returns status (queued/processing/completed/failed/expired) and, when completed, the transcript. `expired` means the job finished more than an hour ago and its transcript has been released; an id that was never issued is an error instead."
    )]
    async fn transcribe_status(
        &self,
        Parameters(p): Parameters<TranscribeStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        match crate::control_api::lookup_transcribe_job(&p.job_id).await {
            Some(job) => json_result(&job),
            None => Err(core_err(format!("job {} not found", p.job_id))),
        }
    }

    #[tool(
        description = "Get Thoth's current pipeline state (idle, recording, or transcribing). An unfinished `transcribe_file` job counts as transcribing."
    )]
    async fn get_state(&self) -> Result<CallToolResult, McpError> {
        let state = crate::pipeline::get_pipeline_state();

        // Background file transcription runs off the live dictation pipeline and
        // never touches its counter, so `get_pipeline_state()` reports `Idle`
        // throughout. Without this an agent that had just submitted a job and
        // asked what Thoth was doing was told "nothing".
        let state = if state == crate::pipeline::PipelineState::Idle
            && !crate::control_api::active_transcribe_jobs()
                .await
                .is_empty()
        {
            crate::pipeline::PipelineState::Transcribing
        } else {
            state
        };

        json_result(&state)
    }

    #[tool(description = "Get GPU/system info and transcription readiness.")]
    async fn get_system(&self) -> Result<CallToolResult, McpError> {
        let info = crate::platform::get_gpu_info().map_err(|e| core_err(e.to_string()))?;
        json_result(&info)
    }

    #[tool(description = "List the available AI-enhancement prompt templates.")]
    async fn list_prompts(&self) -> Result<CallToolResult, McpError> {
        let prompts = crate::enhancement::prompts::get_all_prompts();
        json_result(&prompts)
    }

    #[tool(
        description = "Control recording the same way the global hotkey does: `start`, `stop`, or `toggle`. `stop` (and a toggle that stops) runs the full transcription pipeline on what was recorded, honouring your saved filter, spelling and enhancement settings, then inserts/copies the text per your settings. Returns the resulting action (and the recording path on a start)."
    )]
    async fn recording(
        &self,
        Parameters(p): Parameters<RecordingParams>,
    ) -> Result<CallToolResult, McpError> {
        let app = crate::app_handle::get()
            .ok_or_else(|| core_err("Thoth app handle is not available yet".into()))?;
        match p.action.as_str() {
            "start" => {
                let path = crate::pipeline::pipeline_start_recording(app)
                    .map_err(|e| core_err(e.to_string()))?;
                json_result(&serde_json::json!({ "action": "started", "path": path }))
            }
            "stop" => {
                let cfg = crate::pipeline::effective_pipeline_config()
                    .map_err(|e| core_err(e.to_string()))?;
                crate::pipeline::pipeline_stop_and_process(app, Some(cfg))
                    .await
                    .map_err(|e| core_err(e.to_string()))?;
                json_result(&serde_json::json!({ "action": "stopped" }))
            }
            "toggle" => {
                let cfg = crate::pipeline::effective_pipeline_config()
                    .map_err(|e| core_err(e.to_string()))?;
                let outcome = crate::pipeline::pipeline_toggle_recording(app, Some(cfg), None)
                    .await
                    .map_err(|e| core_err(e.to_string()))?;
                json_result(&outcome)
            }
            other => Err(core_err(format!(
                "unknown action '{}'; must be start | stop | toggle",
                other
            ))),
        }
    }
}

#[tool_handler]
impl ServerHandler for ThothMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Use this server to read and control the locally-running Thoth voice-transcription \
                 app on this machine. Dispatchers: `dictionary` (list/add/update/delete/import/export), \
                 `setting` (get/update), `transcription` (list/get/stats). Singletons: `transcribe_file` \
                 + `transcribe_status` (transcribe a local audio file as a background job), \
                 `recording` (start/stop/toggle, mirroring the global hotkey), `get_state`, \
                 `get_system`, `list_prompts`. All operations \
                 mirror what the user can do in Thoth's GUI; genuinely destructive or system-level \
                 operations (deleting history, quitting the app) remain unexposed. This controls only the \
                 local instance."
                    .to_string(),
            )
    }
}

// ---------------------------------------------------------------------------
// Transport service (mounted on the Control API's axum router at /mcp)
// ---------------------------------------------------------------------------

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

/// Build the streamable-HTTP MCP service for mounting on the loopback axum router.
///
/// `allowed_hosts` defaults to loopback only (anti DNS-rebinding); the Control API's
/// bearer-token auth layer is applied in front of the mount point.
pub fn build_service() -> StreamableHttpService<ThothMcp, LocalSessionManager> {
    StreamableHttpService::new(
        || Ok(ThothMcp::new()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    )
}
