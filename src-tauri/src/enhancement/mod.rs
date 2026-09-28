//! AI text enhancement subsystem
//!
//! Provides AI-powered text enhancement using local LLM backends:
//! - Ollama (default)
//! - Any OpenAI-compatible endpoint (LM Studio, llama.cpp server, vLLM, etc.)

pub mod context;
pub mod ollama;
pub mod openai_compat;
pub mod prompts;

pub use context::{
    ContextCapture, build_context, build_enhancement_context, get_clipboard_context,
};
pub use ollama::OllamaClient;
pub use openai_compat::OpenAiCompatClient;
pub use prompts::{
    PromptTemplate, delete_custom_prompt_cmd, get_all_prompts, get_builtin_prompts_cmd,
    get_custom_prompts_cmd, get_prompt_by_id, save_custom_prompt_cmd,
};

use crate::TELEMETRY_TARGET;
use crate::error::Error;
use parking_lot::Mutex;
use std::sync::OnceLock;

/// One attempt, including response decoding, before retaining the original
/// text. Covers a cold model load, not just generation: Ollama loads a model
/// as part of handling the first request after it was unloaded (idle timeout,
/// or just after Thoth started) rather than on a separate warm-up call, and
/// with the 3-attempt retry gone there is no second chance if this is too
/// short. A genuinely unreachable server fails long before this — the shared
/// HTTP client's own connect timeout (telemetry-rs) catches that in ~10s —
/// so this bound is only ever reached by a connection that is open but slow
/// to answer.
const REQUEST_TIMEOUT_SECS: u64 = 90;

/// Above this, a small local model reliably mishandles the rewrite (#179: a
/// 9,683-char dictation came back at 7-19% of its own length in testing) and
/// a correctly sized reply would run well past REQUEST_TIMEOUT_SECS anyway.
/// Skipping is instant; the filtered transcript still pastes.
const MAX_ENHANCE_INPUT_CHARS: usize = 6_000;

/// Exact `Error::Other` text for the two `enhance_text` outcomes the pipeline
/// surfaces to the user as a toast: the household's existing sentinel-string
/// pattern for a Tauri-command error the frontend branches on, same as the
/// no-speech and Input Monitoring sentinels (`error.rs`'s module docs). A
/// network or model error is not one of these: it logs and falls back
/// without a toast, which is a separate, deliberately quieter path.
pub const ENHANCEMENT_SKIPPED_TOO_LONG: &str = "Dictation too long to enhance";
pub const ENHANCEMENT_OUTPUT_IMPLAUSIBLE: &str = "Enhancement output length was implausible";

/// A deliberately low token-per-char estimate (English averages nearer 4), so
/// a request is never undersized for text that tokenises more densely.
fn estimate_tokens(chars: usize) -> usize {
    chars / 3 + 16
}

/// Bounds a single generation. A grammar fix rarely grows text; capped well
/// above 1:1 so a legitimate long dictation still fits, without leaving a
/// model that loops instead of stopping free to run for minutes.
fn output_token_limit(text: &str) -> usize {
    (estimate_tokens(text.len()) * 3 / 2).clamp(256, 4096)
}

/// A model that shrinks or runs away past these bounds is misbehaving, not
/// improving the text — `enhance_text` rejects it and the pipeline already
/// retains the original on any `Err` from here, which is strictly safer than
/// pasting a truncated or runaway reply. This is the default band, for a
/// prompt whose intent is an approximately same-length rewrite; a prompt that
/// deliberately changes length (summarise, expand) gets its own band from
/// [`expected_output_ratio`].
const MIN_OUTPUT_RATIO: f64 = 0.5;
const MAX_OUTPUT_RATIO: f64 = 2.5;
const MIN_INPUT_CHARS_FOR_RATIO_CHECK: usize = 50;

/// The plausible output/input length ratio for a prompt, by its shipped
/// intent. Matched against the fixed built-in template text — never
/// user-edited, since a built-in prompt cannot be modified — so a custom
/// prompt always falls through to the generic same-length band.
///
/// Without this, the generic band rejects the entire point of these two
/// prompts: `summarise` asks for "1-2 sentences" from a whole dictation
/// (routinely well under 0.5x), and `expand` asks for "2-3x more detail"
/// (routinely over 2.5x), so both would silently fall back to pasting the
/// unenhanced original on every single use.
fn expected_output_ratio(prompt_template: &str) -> (f64, f64) {
    let matched_id = prompts::get_builtin_prompts()
        .into_iter()
        .find(|p| p.template == prompt_template)
        .map(|p| p.id);

    match matched_id.as_deref() {
        // "1-2 sentences": can legitimately be a small fraction of a long
        // dictation — a couple of sentences from 9,683 chars (#179's own
        // reported length) is already under 2%. Only the upper bound guards
        // anything here — a "summary" longer than its source did not summarise.
        Some("summarise") => (0.01, 1.0),
        // "2-3x more detail" (i.e. up to ~4x total). Capped short of the 8.5x
        // runaway measured in testing (#179), not at the requested 3x, so a
        // slightly generous expansion is not falsely rejected.
        Some("expand") => (1.0, 6.0),
        _ => (MIN_OUTPUT_RATIO, MAX_OUTPUT_RATIO),
    }
}

fn output_length_is_plausible(input_len: usize, output_len: usize, bounds: (f64, f64)) -> bool {
    if input_len < MIN_INPUT_CHARS_FOR_RATIO_CHECK {
        return true;
    }
    let ratio = output_len as f64 / input_len as f64;
    (bounds.0..=bounds.1).contains(&ratio)
}

/// Which enhancement backend is active
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendType {
    Ollama,
    OpenAiCompat,
}

/// Holds the active backend configuration
struct EnhancementBackend {
    backend_type: BackendType,
    ollama: OllamaClient,
    openai_compat: Option<OpenAiCompatClient>,
}

impl EnhancementBackend {
    fn new() -> Self {
        Self {
            backend_type: BackendType::Ollama,
            ollama: OllamaClient::new(),
            openai_compat: None,
        }
    }
}

/// Global enhancement backend instance
static BACKEND: OnceLock<Mutex<EnhancementBackend>> = OnceLock::new();

fn get_backend() -> &'static Mutex<EnhancementBackend> {
    BACKEND.get_or_init(|| Mutex::new(EnhancementBackend::new()))
}

/// Configure the active enhancement backend.
///
/// Called on startup (after config load) and after `set_config` changes the
/// enhancement section. Must be called before the first pipeline run.
///
/// Returns the `BackendType` that was actually activated (callers may request
/// `openai_compat` but receive `Ollama` if the URL is invalid).
///
/// # Arguments
///
/// * `backend` - `"ollama"` or `"openai_compat"` (any other value defaults to Ollama)
/// * `ollama_url` - Ollama base URL (used when backend is Ollama)
/// * `openai_compat_url` - OpenAI-compat base URL (used when backend is openai_compat)
/// * `api_key` - Optional API key for the OpenAI-compat endpoint
pub fn configure_backend(
    backend: &str,
    ollama_url: &str,
    openai_compat_url: &str,
    api_key: Option<&str>,
) -> BackendType {
    let mut b = get_backend().lock();

    // Always update the Ollama client URL
    b.ollama = OllamaClient::with_base_url(ollama_url.to_string());

    match backend {
        "openai_compat" => {
            match OpenAiCompatClient::new(
                openai_compat_url.to_string(),
                api_key.map(|k| k.to_string()),
            ) {
                Ok(client) => {
                    b.backend_type = BackendType::OpenAiCompat;
                    b.openai_compat = Some(client);
                    tracing::info!(
                        "Enhancement backend: OpenAI-compat at {}",
                        openai_compat_url
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "Invalid OpenAI-compat URL '{}', falling back to Ollama: {}",
                        openai_compat_url,
                        e
                    );
                    b.backend_type = BackendType::Ollama;
                    b.openai_compat = None;
                }
            }
        }
        _ => {
            b.backend_type = BackendType::Ollama;
            b.openai_compat = None;
            tracing::info!("Enhancement backend: Ollama at {}", ollama_url);
        }
    }

    b.backend_type
}

// --- Tauri Commands ---

/// Check if the Ollama server is available
#[tauri::command]
pub async fn check_ollama_available() -> bool {
    tauri_plugin_telemetry::traced("check_ollama_available", async move {
        Ok::<_, std::convert::Infallible>({
            let client = get_backend().lock().ollama.clone();
            client.is_available().await
        })
    })
    .await
    .unwrap()
}

/// List available Ollama models
#[tauri::command]
pub async fn list_ollama_models() -> Result<Vec<String>, Error> {
    tauri_plugin_telemetry::traced("list_ollama_models", async move {
        let client = get_backend().lock().ollama.clone();
        client
            .list_models()
            .await
            .map_err(|e| {
                tracing::error!("Failed to list Ollama models: {}", e);
                format!("Failed to list models: {}", e)
            })
            .map_err(Into::into)
    })
    .await
}

/// Check if the configured OpenAI-compatible server is available
#[tauri::command]
pub async fn check_openai_compat_available() -> bool {
    tauri_plugin_telemetry::traced("check_openai_compat_available", async move {
        Ok::<_, std::convert::Infallible>({
            let client = get_backend().lock().openai_compat.clone();
            match client {
                Some(c) => c.is_available().await,
                None => false,
            }
        })
    })
    .await
    .unwrap()
}

/// List available models from the OpenAI-compatible server
#[tauri::command]
pub async fn list_openai_compat_models() -> Result<Vec<String>, Error> {
    tauri_plugin_telemetry::traced("list_openai_compat_models", async move {
        let client = get_backend().lock().openai_compat.clone();
        match client {
            Some(c) => c
                .list_models()
                .await
                .map_err(|e| {
                    tracing::error!("Failed to list OpenAI-compat models: {}", e);
                    format!("Failed to list models: {}", e)
                })
                .map_err(Into::into),
            None => Err("OpenAI-compatible backend not configured"
                .to_string()
                .into()),
        }
    })
    .await
}

/// Enhance text using the active backend.
///
/// The prompt template must contain `{text}`, which is substituted with the
/// transcript in-place before being sent as the sole user message. Both the
/// Ollama and OpenAI-compat backends use this single-message format.
///
/// The public signature `(text, model, prompt)` is unchanged; only internal
/// dispatch changed.
#[tauri::command]
#[tracing::instrument(
    target = TELEMETRY_TARGET,
    name = "enhancement",
    skip_all,
    fields(
        model = %model,
        backend = tracing::field::Empty,
        input_bytes = text.len(),
        output_bytes = tracing::field::Empty,
        ok = tracing::field::Empty,
    )
)]
pub async fn enhance_text(text: String, model: String, prompt: String) -> Result<String, Error> {
    // Bound before the early returns: a call rejected here must still export an
    // outcome, not a span whose `ok` was never set.
    let span = tracing::Span::current();

    if text.is_empty() {
        span.record("ok", false);
        return Err("Text cannot be empty".to_string().into());
    }

    if model.is_empty() {
        span.record("ok", false);
        return Err("Model cannot be empty".to_string().into());
    }

    if text.len() > MAX_ENHANCE_INPUT_CHARS {
        span.record("ok", false);
        tracing::info!(
            "Skipping enhancement for a {}-char dictation (over the {}-char cap); pasting the filtered transcript",
            text.len(),
            MAX_ENHANCE_INPUT_CHARS
        );
        return Err(Error::Other(ENHANCEMENT_SKIPPED_TOO_LONG.into()));
    }

    let (backend_type, ollama, openai_compat) = {
        let b = get_backend().lock();
        (b.backend_type, b.ollama.clone(), b.openai_compat.clone())
    };
    span.record("backend", tracing::field::debug(backend_type));

    tracing::info!(
        "Enhancing text with model '{}' ({} chars, backend: {:?})",
        model,
        text.len(),
        backend_type
    );

    let result = match backend_type {
        BackendType::Ollama => ollama
            .enhance_text(&text, &model, &prompt)
            .await
            .map_err(|e| {
                span.record("ok", false);
                tracing::error!("Ollama enhancement failed: {}", e);
                format!("Enhancement failed: {}", e)
            })?,
        BackendType::OpenAiCompat => {
            let client = openai_compat.ok_or_else(|| {
                span.record("ok", false);
                "OpenAI-compatible backend not configured".to_string()
            })?;
            client
                .enhance_text(&text, &model, &prompt)
                .await
                .map_err(|e| {
                    span.record("ok", false);
                    tracing::error!("OpenAI-compat enhancement failed: {}", e);
                    format!("Enhancement failed: {}", e)
                })?
        }
    };

    if result.trim().is_empty() {
        span.record("ok", false);
        return Err(Error::Other("Enhancement returned no text".into()));
    }

    if !output_length_is_plausible(text.len(), result.len(), expected_output_ratio(&prompt)) {
        span.record("ok", false);
        tracing::warn!(
            "Enhancement output length implausible ({} -> {} chars), discarding",
            text.len(),
            result.len()
        );
        return Err(Error::Other(ENHANCEMENT_OUTPUT_IMPLAUSIBLE.into()));
    }

    span.record("output_bytes", result.len());
    span.record("ok", true);
    tracing::info!(
        "Enhancement complete ({} -> {} characters)",
        text.len(),
        result.len()
    );

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Serialises tests that call configure_backend so they cannot interleave on
    /// the shared BACKEND singleton. Each test asserts only on the BackendType
    /// returned by configure_backend (which is the value it just wrote), so the
    /// guard is sufficient to avoid both write-write and read-write races.
    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    #[test]
    fn test_backend_initialises_as_ollama() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let bt = configure_backend(
            "ollama",
            "http://localhost:11434",
            "http://localhost:1234",
            None,
        );
        assert_eq!(bt, BackendType::Ollama);
    }

    #[test]
    fn test_configure_backend_switches_to_openai_compat() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let bt = configure_backend(
            "openai_compat",
            "http://localhost:11434",
            "http://localhost:1234",
            None,
        );
        assert_eq!(bt, BackendType::OpenAiCompat);
    }

    #[test]
    fn test_configure_backend_invalid_url_falls_back_to_ollama() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let bt = configure_backend(
            "openai_compat",
            "http://localhost:11434",
            "file:///bad-scheme",
            None,
        );
        assert_eq!(bt, BackendType::Ollama);
    }

    const GENERIC_BOUNDS: (f64, f64) = (MIN_OUTPUT_RATIO, MAX_OUTPUT_RATIO);

    #[test]
    fn test_output_length_is_plausible_within_bounds() {
        assert!(output_length_is_plausible(9683, 8500, GENERIC_BOUNDS));
        assert!(output_length_is_plausible(1500, 1382, GENERIC_BOUNDS));
    }

    #[test]
    fn test_output_length_rejects_the_179_truncation() {
        // The exact failure from #179: a 9,683-char dictation returned 1,807 chars.
        assert!(!output_length_is_plausible(9683, 1807, GENERIC_BOUNDS));
    }

    #[test]
    fn test_output_length_rejects_a_runaway_reply() {
        assert!(!output_length_is_plausible(9686, 82655, GENERIC_BOUNDS));
    }

    #[test]
    fn test_output_length_skips_the_check_for_tiny_input() {
        // A one-word dictation can legitimately shrink to nothing after filler
        // removal; the ratio check would otherwise flag every short command.
        assert!(output_length_is_plausible(10, 1, GENERIC_BOUNDS));
    }

    /// Runs the ratio check against every shipped prompt's own intended output
    /// shape, so a prompt whose whole point is to change length is never
    /// silently defeated by the plausibility gate meant to catch truncation.
    #[test]
    fn test_expected_output_ratio_matches_each_builtin_prompts_own_intent() {
        let dictation_len = 9_683; // #179's own reported dictation length.

        for prompt in prompts::get_builtin_prompts() {
            let bounds = expected_output_ratio(&prompt.template);
            let (legitimate_output_len, description): (usize, &str) = match prompt.id.as_str() {
                // "1-2 sentences": a couple of sentences out of a long dictation.
                "summarise" => (180, "a 1-2 sentence summary"),
                // "2-3x more detail": ~3.2x the input.
                "expand" => (31_000, "a 2-3x expansion"),
                // Every other built-in asks to keep approximately the same length.
                _ => (
                    (dictation_len as f64 * 0.9) as usize,
                    "an approximately same-length rewrite",
                ),
            };

            assert!(
                output_length_is_plausible(dictation_len, legitimate_output_len, bounds),
                "{}'s own legitimate output ({description}) was rejected by its bounds {bounds:?}",
                prompt.id,
            );
        }
    }

    #[test]
    fn test_expected_output_ratio_summarise_still_rejects_a_longer_than_input_reply() {
        let bounds = expected_output_ratio(
            &prompts::get_builtin_prompts()
                .into_iter()
                .find(|p| p.id == "summarise")
                .unwrap()
                .template,
        );
        // A "summary" that comes back longer than its source did not summarise.
        assert!(!output_length_is_plausible(9683, 11_000, bounds));
    }

    #[test]
    fn test_expected_output_ratio_expand_still_rejects_a_179_style_collapse() {
        let bounds = expected_output_ratio(
            &prompts::get_builtin_prompts()
                .into_iter()
                .find(|p| p.id == "expand")
                .unwrap()
                .template,
        );
        // Asking to expand and getting back a fraction of the input is still
        // the truncation failure, not a legitimate (if modest) expansion.
        assert!(!output_length_is_plausible(9683, 1807, bounds));
    }

    #[test]
    fn test_expected_output_ratio_falls_back_to_generic_for_a_custom_prompt() {
        assert_eq!(
            expected_output_ratio("Translate the following to French:\n\n{text}"),
            GENERIC_BOUNDS
        );
    }

    #[test]
    fn test_output_token_limit_covers_a_1to1_rewrite_of_the_cap() {
        let text = "a".repeat(MAX_ENHANCE_INPUT_CHARS);
        let estimated_output_tokens = output_token_limit(&text);
        // A verbatim-length correction of the longest input we still attempt
        // must fit the token budget we ask Ollama for.
        assert!(estimated_output_tokens as f64 >= estimate_tokens(text.len()) as f64 * 0.9);
    }

    #[test]
    fn test_configure_backend_unknown_backend_defaults_to_ollama() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let bt = configure_backend(
            "unknown_backend",
            "http://localhost:11434",
            "http://localhost:1234",
            None,
        );
        assert_eq!(bt, BackendType::Ollama);
    }
}
