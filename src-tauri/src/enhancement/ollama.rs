//! Ollama HTTP client for AI text enhancement
//!
//! Provides local AI enhancement via the Ollama API running at localhost:11434.
//! Each generation makes one bounded request.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::{REQUEST_TIMEOUT_SECS, estimate_tokens, output_token_limit};

/// Default Ollama server address
const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434";

/// Request body for Ollama generate endpoint
#[derive(Debug, Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    options: GenerateOptions,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct GenerateOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    num_predict: usize,
    num_ctx: usize,
}

/// Left unset, Ollama defaults num_ctx far below a long dictation's prompt
/// and silently drops the *start* of whatever overruns it rather than
/// erroring — the model never sees the missing part, which is what produced
/// #179's truncated replies, not the output length. Sized to the whole
/// prompt plus the reply budget, so nothing is ever dropped.
fn context_window(prompt: &str) -> usize {
    estimate_tokens(prompt.len()) + output_token_limit(prompt) + 512
}

/// Response from Ollama generate endpoint (non-streaming)
#[derive(Debug, Deserialize)]
struct GenerateResponse {
    response: String,
    #[serde(default)]
    #[allow(dead_code)]
    done: bool,
}

/// Response from Ollama tags endpoint
#[derive(Debug, Deserialize)]
struct TagsResponse {
    models: Vec<ModelInfo>,
}

/// Model information from Ollama
#[derive(Debug, Deserialize)]
struct ModelInfo {
    name: String,
}

/// Error types for Ollama operations
#[derive(Debug, thiserror::Error)]
pub enum OllamaError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Request timeout after {0} seconds")]
    Timeout(u64),

    #[error("Server error ({status}): {message}")]
    ServerError { status: u16, message: String },

    #[error("Failed to parse response: {0}")]
    ParseError(String),
}

/// Ollama HTTP client for AI text enhancement
///
/// Supports a configurable base URL and timeout.
#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    client: reqwest_middleware::ClientWithMiddleware,
    timeout: Duration,
    default_model: Option<String>,
}

impl Default for OllamaClient {
    fn default() -> Self {
        Self::new()
    }
}

impl OllamaClient {
    /// Create a new Ollama client with default settings
    pub fn new() -> Self {
        Self::with_config(DEFAULT_OLLAMA_BASE_URL, REQUEST_TIMEOUT_SECS, None)
    }

    /// Create a new Ollama client with a custom base URL
    pub fn with_base_url(base_url: String) -> Self {
        Self::with_config(&base_url, REQUEST_TIMEOUT_SECS, None)
    }

    /// Create a new Ollama client with full configuration
    ///
    /// # Arguments
    ///
    /// * `base_url` - The Ollama server base URL (e.g., "http://localhost:11434")
    /// * `timeout_secs` - Request timeout in seconds
    /// * `default_model` - Optional default model to use if not specified per-request
    pub fn with_config(base_url: &str, timeout_secs: u64, default_model: Option<String>) -> Self {
        let timeout = Duration::from_secs(timeout_secs);
        crate::ensure_crypto_provider();

        Self {
            base_url: base_url.to_string(),
            // The process's shared traceparent-carrying client. It has no
            // client-level timeout, so every request below sets its own.
            client: crate::http_client(),
            timeout,
            default_model,
        }
    }

    /// Set the default model for this client
    pub fn set_default_model(&mut self, model: impl Into<String>) {
        self.default_model = Some(model.into());
    }

    /// Get the configured timeout
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Check if Ollama server is available
    pub async fn is_available(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        match self.client.get(&url).timeout(self.timeout).send().await {
            Ok(response) => response.status().is_success(),
            Err(e) => {
                tracing::debug!("Ollama not available: {}", e);
                false
            }
        }
    }

    /// List available models from Ollama
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/api/tags", self.base_url);

        let response = self
            .client
            .get(&url)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to connect to Ollama: {}", e))?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Ollama returned error status: {}",
                response.status()
            ));
        }

        let tags: TagsResponse = response
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse Ollama response: {}", e))?;

        let model_names: Vec<String> = tags.models.into_iter().map(|m| m.name).collect();

        tracing::debug!("Found {} Ollama models", model_names.len());
        Ok(model_names)
    }

    /// Send a single generate request (internal helper)
    async fn send_generate_request(
        &self,
        request: &GenerateRequest,
    ) -> Result<String, OllamaError> {
        let url = format!("{}/api/generate", self.base_url);

        let response = self
            .client
            .post(&url)
            .json(request)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    OllamaError::Timeout(self.timeout.as_secs())
                } else {
                    OllamaError::ConnectionFailed(e.to_string())
                }
            })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(OllamaError::ServerError { status, message });
        }

        let generate_response: GenerateResponse = response
            .json()
            .await
            .map_err(|e| OllamaError::ParseError(e.to_string()))?;

        Ok(generate_response.response)
    }

    /// Generate text using the specified model with retry logic
    ///
    /// Retries up to 3 times with exponential backoff (100ms, 200ms, 400ms).
    pub async fn generate(&self, model: &str, prompt: &str) -> Result<String> {
        self.generate_with_system(model, prompt, None, None).await
    }

    /// Generate text with a system prompt and optional temperature
    ///
    /// # Arguments
    ///
    /// * `model` - The model name to use
    /// * `prompt` - The user prompt text
    /// * `system_prompt` - Optional system prompt for context
    /// * `temperature` - Optional temperature (0.0 to 1.0)
    pub async fn generate_with_system(
        &self,
        model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        temperature: Option<f32>,
    ) -> Result<String> {
        let request = GenerateRequest {
            model: model.to_string(),
            prompt: prompt.to_string(),
            system: system_prompt.map(|s| s.to_string()),
            options: GenerateOptions {
                temperature,
                num_predict: output_token_limit(prompt),
                num_ctx: context_window(prompt),
            },
            stream: false,
        };

        tracing::debug!(
            "Sending generate request to Ollama with model: {} (system prompt: {})",
            model,
            system_prompt.is_some()
        );

        Ok(
            tokio::time::timeout(self.timeout, self.send_generate_request(&request))
                .await
                .map_err(|_| OllamaError::Timeout(self.timeout.as_secs()))??,
        )
    }

    /// Enhance text using the specified model and prompt template
    ///
    /// The prompt template should contain `{text}` which will be replaced with the input text.
    pub async fn enhance_text(
        &self,
        text: &str,
        model: &str,
        prompt_template: &str,
    ) -> Result<String> {
        let full_prompt = prompt_template.replace("{text}", text);
        self.generate(model, &full_prompt).await
    }

    /// Enhance text with a system prompt
    ///
    /// This wraps the text in a TRANSCRIPT tag and uses the system prompt for context.
    pub async fn enhance_with_system(
        &self,
        text: &str,
        model: &str,
        system_prompt: &str,
    ) -> Result<String> {
        let prompt = format!("<TRANSCRIPT>\n{}\n</TRANSCRIPT>", text);
        self.generate_with_system(model, &prompt, Some(system_prompt), Some(0.3))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = OllamaClient::new();
        assert_eq!(client.base_url, DEFAULT_OLLAMA_BASE_URL);
        assert_eq!(client.timeout.as_secs(), REQUEST_TIMEOUT_SECS);
    }

    #[test]
    fn test_client_with_custom_url() {
        let custom_url = "http://custom:8080".to_string();
        let client = OllamaClient::with_base_url(custom_url.clone());
        assert_eq!(client.base_url, custom_url);
    }

    #[test]
    fn test_client_with_config() {
        let client =
            OllamaClient::with_config("http://example.com:11434", 60, Some("llama3.2".to_string()));
        assert_eq!(client.base_url, "http://example.com:11434");
        assert_eq!(client.timeout.as_secs(), 60);
        assert_eq!(client.default_model, Some("llama3.2".to_string()));
    }

    #[test]
    fn test_default_impl() {
        let client = OllamaClient::default();
        assert_eq!(client.base_url, DEFAULT_OLLAMA_BASE_URL);
    }

    #[test]
    fn test_generate_request_serialisation_basic() {
        let request = GenerateRequest {
            model: "llama3.2".to_string(),
            prompt: "test prompt".to_string(),
            system: None,
            options: GenerateOptions {
                temperature: None,
                num_predict: 256,
                num_ctx: 4096,
            },
            stream: false,
        };

        let json = serde_json::to_string(&request).expect("Failed to serialise");
        assert!(json.contains("\"model\":\"llama3.2\""));
        assert!(json.contains("\"stream\":false"));
        assert!(json.contains("\"num_predict\":256"));
        assert!(json.contains("\"num_ctx\":4096"));
        // system and temperature should be omitted when None
        assert!(!json.contains("\"system\""));
        assert!(!json.contains("\"temperature\""));
    }

    #[test]
    fn test_generate_request_serialisation_with_system() {
        let request = GenerateRequest {
            model: "llama3.2".to_string(),
            prompt: "test prompt".to_string(),
            system: Some("You are a helpful assistant.".to_string()),
            options: GenerateOptions {
                temperature: Some(0.3),
                num_predict: 256,
                num_ctx: 4096,
            },
            stream: false,
        };

        let json = serde_json::to_string(&request).expect("Failed to serialise");
        assert!(json.contains("\"system\":\"You are a helpful assistant.\""));
        assert!(json.contains("\"temperature\":0.3"));
    }

    #[test]
    fn test_error_display() {
        let err = OllamaError::ConnectionFailed("connection refused".to_string());
        assert_eq!(err.to_string(), "Connection failed: connection refused");

        let err = OllamaError::Timeout(30);
        assert_eq!(err.to_string(), "Request timeout after 30 seconds");

        let err = OllamaError::ServerError {
            status: 500,
            message: "Internal error".to_string(),
        };
        assert_eq!(err.to_string(), "Server error (500): Internal error");
    }

    #[test]
    fn test_context_window_covers_the_whole_prompt() {
        let prompt = "a".repeat(9686);
        let window_tokens = context_window(&prompt);
        assert!(window_tokens > estimate_tokens(prompt.len()));
    }

    #[test]
    fn test_set_default_model() {
        let mut client = OllamaClient::new();
        assert!(client.default_model.is_none());

        client.set_default_model("llama3.2");
        assert_eq!(client.default_model, Some("llama3.2".to_string()));
    }

    #[test]
    fn test_timeout_getter() {
        let client = OllamaClient::with_config("http://localhost:11434", 45, None);
        assert_eq!(client.timeout().as_secs(), 45);
    }
}
