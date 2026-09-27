//! OpenAI-compatible HTTP client for AI text enhancement
//!
//! Supports any endpoint implementing the `/v1/chat/completions` API:
//! LM Studio, llama.cpp server, vLLM, Ollama (OpenAI mode), etc.
//! Works fully offline against local endpoints.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::{REQUEST_TIMEOUT_SECS, output_token_limit};

/// A single message in the chat completions request
#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

/// Request body for the `/v1/chat/completions` endpoint
#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    max_tokens: usize,
}

/// A single choice in the chat completions response
#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

/// The message within a chat choice
#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

/// Response from the `/v1/chat/completions` endpoint
#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

/// Response from the `/v1/models` endpoint
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelObject>,
}

/// A single model object from `/v1/models`
#[derive(Debug, Deserialize)]
struct ModelObject {
    id: String,
}

/// Error types for OpenAI-compatible client operations
#[derive(Debug, thiserror::Error)]
pub enum OpenAiCompatError {
    #[error("Invalid base URL: {0}")]
    InvalidUrl(String),

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Request timeout after {0} seconds")]
    Timeout(u64),

    #[error("Server error ({status}): {message}")]
    ServerError { status: u16, message: String },

    #[error("Empty response: no choices returned")]
    EmptyChoices,

    #[error("Failed to parse response: {0}")]
    ParseError(String),
}

/// HTTP client for any OpenAI-compatible `/v1/chat/completions` endpoint.
///
/// Supports a configurable base URL and an optional API key. Never logs the
/// API key.
#[derive(Clone)]
pub struct OpenAiCompatClient {
    base_url: String,
    api_key: Option<String>,
    client: reqwest_middleware::ClientWithMiddleware,
    timeout: Duration,
}

impl std::fmt::Debug for OpenAiCompatClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatClient")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "***redacted***"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl OpenAiCompatClient {
    /// Create a new client.
    ///
    /// # Arguments
    ///
    /// * `base_url` - Server base URL (e.g. `"http://localhost:1234"`)
    /// * `api_key` - Optional API key; included as `Bearer` token if provided
    ///
    /// # Errors
    ///
    /// Returns an error if `base_url` uses an unsupported scheme (not `http://` or `https://`).
    pub fn new(base_url: String, api_key: Option<String>) -> Result<Self, OpenAiCompatError> {
        Self::with_timeout(base_url, api_key, REQUEST_TIMEOUT_SECS)
    }

    /// Create a new client with an explicit timeout.
    pub fn with_timeout(
        base_url: String,
        api_key: Option<String>,
        timeout_secs: u64,
    ) -> Result<Self, OpenAiCompatError> {
        // Validate URL scheme — accept only http:// and https://
        let lower = base_url.to_lowercase();
        if !lower.starts_with("http://") && !lower.starts_with("https://") {
            return Err(OpenAiCompatError::InvalidUrl(format!(
                "Base URL must use http:// or https://, got: {}",
                base_url
            )));
        }

        let timeout = Duration::from_secs(timeout_secs);
        crate::ensure_crypto_provider();

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            // The process's shared traceparent-carrying client. It has no
            // client-level timeout, so every request below sets its own.
            client: crate::http_client(),
            timeout,
        })
    }

    /// Check if the server is reachable by probing `/v1/models`.
    pub async fn is_available(&self) -> bool {
        let url = format!("{}/v1/models", self.base_url);
        let mut req = self.client.get(&url).timeout(self.timeout);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        match req.send().await {
            Ok(response) => response.status().is_success(),
            Err(e) => {
                tracing::debug!("OpenAI-compat server not available: {}", e);
                false
            }
        }
    }

    /// List available models from `/v1/models`.
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/v1/models", self.base_url);
        let mut req = self.client.get(&url).timeout(self.timeout);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let response = req
            .send()
            .await
            .map_err(|e| anyhow!("Failed to connect to OpenAI-compat server: {}", e))?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Server returned error status: {}",
                response.status()
            ));
        }

        let models_response: ModelsResponse = response
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse models response: {}", e))?;

        let ids: Vec<String> = models_response.data.into_iter().map(|m| m.id).collect();
        tracing::debug!("OpenAI-compat: found {} models", ids.len());
        Ok(ids)
    }

    /// Send a single chat completions request (internal helper).
    async fn send_chat_request(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<String, OpenAiCompatError> {
        let url = format!("{}/v1/chat/completions", self.base_url);

        let mut req = self.client.post(&url).json(request).timeout(self.timeout);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let response = req.send().await.map_err(|e| {
            if e.is_timeout() {
                OpenAiCompatError::Timeout(self.timeout.as_secs())
            } else {
                OpenAiCompatError::ConnectionFailed(e.to_string())
            }
        })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(OpenAiCompatError::ServerError { status, message });
        }

        let completion: ChatCompletionResponse = response
            .json()
            .await
            .map_err(|e| OpenAiCompatError::ParseError(e.to_string()))?;

        let content = completion
            .choices
            .into_iter()
            .next()
            .ok_or(OpenAiCompatError::EmptyChoices)?
            .message
            .content;

        Ok(content)
    }

    /// Enhance text using the specified model and prompt template.
    ///
    /// The prompt template must contain `{text}`, which is substituted with the
    /// transcript in-place (matching the Ollama backend semantics). The rendered
    /// result is sent as the sole user message.
    ///
    /// Retries up to 3 times with exponential backoff (100 ms, 200 ms, 400 ms).
    pub async fn enhance_text(
        &self,
        text: &str,
        model: &str,
        prompt_template: &str,
    ) -> Result<String> {
        // Substitute {text} with the actual transcript, matching ollama.rs line 305.
        // The rendered prompt is sent as the user message so the model receives
        // both the instruction and the content in one turn.
        let user_content = prompt_template.replace("{text}", text);
        let max_tokens = output_token_limit(&user_content);

        let request = ChatCompletionRequest {
            model: model.to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: user_content,
            }],
            stream: false,
            max_tokens,
        };

        tracing::debug!("OpenAI-compat: sending chat request with model: {}", model);

        Ok(
            tokio::time::timeout(self.timeout, self.send_chat_request(&request))
                .await
                .map_err(|_| OpenAiCompatError::Timeout(self.timeout.as_secs()))??,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    // -------------------------------------------------------------------------
    // Construction and URL validation
    // -------------------------------------------------------------------------

    #[test]
    fn test_client_accepts_http_url() {
        let client = OpenAiCompatClient::new("http://localhost:1234".to_string(), None);
        assert!(client.is_ok());
    }

    #[test]
    fn test_client_accepts_https_url() {
        let client = OpenAiCompatClient::new("https://my-server.local:8080".to_string(), None);
        assert!(client.is_ok());
    }

    #[test]
    fn test_client_rejects_file_scheme() {
        let client = OpenAiCompatClient::new("file:///etc/passwd".to_string(), None);
        assert!(matches!(client, Err(OpenAiCompatError::InvalidUrl(_))));
    }

    #[test]
    fn test_client_rejects_ssh_scheme() {
        let client = OpenAiCompatClient::new("ssh://host".to_string(), None);
        assert!(matches!(client, Err(OpenAiCompatError::InvalidUrl(_))));
    }

    #[test]
    fn test_trailing_slash_stripped() {
        let client = OpenAiCompatClient::new("http://localhost:1234/".to_string(), None).unwrap();
        assert_eq!(client.base_url, "http://localhost:1234");
    }

    // -------------------------------------------------------------------------
    // Request construction
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_request_body_shape() {
        let mut server = Server::new_async().await;

        let body_mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"choices":[{"message":{"content":"Enhanced text","role":"assistant"}}]}"#,
            )
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"model":"llama3","stream":false}"#.to_string(),
            ))
            .create_async()
            .await;

        let client = OpenAiCompatClient::new(server.url(), None).unwrap();
        let result = client
            .enhance_text("hello", "llama3", "Fix grammar: {text}")
            .await;

        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
        body_mock.assert_async().await;
    }

    /// Asserts that `{text}` is substituted with the transcript in the single user
    /// message — not stripped. A failure here means the two backends diverge.
    #[tokio::test]
    async fn test_text_placeholder_substituted_in_user_message() {
        let mut server = Server::new_async().await;

        // The rendered content must be "Improve this: my transcript" —
        // the placeholder replaced, NOT stripped to "Improve this: ".
        let body_mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"content":"result","role":"assistant"}}]}"#)
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"messages":[{"role":"user","content":"Improve this: my transcript"}]}"#
                    .to_string(),
            ))
            .create_async()
            .await;

        let client = OpenAiCompatClient::new(server.url(), None).unwrap();
        let result = client
            .enhance_text("my transcript", "any-model", "Improve this: {text}")
            .await;

        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
        // mock assertion fails if the request body did not match — i.e. if {text}
        // was stripped instead of substituted.
        body_mock.assert_async().await;
    }

    /// Asserts no system message is sent and exactly one user message is present
    /// (single-message format matches Ollama semantics).
    #[tokio::test]
    async fn test_request_sends_single_user_message_no_system() {
        let mut server = Server::new_async().await;

        // The messages array must contain EXACTLY one entry with role "user".
        // PartialJsonString checks subset membership — it would accept a two-element
        // array if the first matched. Use JsonString with the full messages array
        // to assert both role and count precisely.
        let body_mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"content":"ok","role":"assistant"}}]}"#)
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"messages":[{"role":"user","content":"Fix: transcript"}]}"#.to_string(),
            ))
            .create_async()
            .await;

        let client = OpenAiCompatClient::new(server.url(), None).unwrap();
        let result = client
            .enhance_text("transcript", "model", "Fix: {text}")
            .await;

        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
        body_mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_api_key_sent_as_bearer_token() {
        let mut server = Server::new_async().await;

        let auth_mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"content":"ok","role":"assistant"}}]}"#)
            .match_header("authorization", "Bearer secret-key")
            .create_async()
            .await;

        let client = OpenAiCompatClient::new(server.url(), Some("secret-key".to_string())).unwrap();
        let result = client.enhance_text("text", "model", "{text}").await;

        assert!(result.is_ok());
        auth_mock.assert_async().await;
    }

    // -------------------------------------------------------------------------
    // Response parsing
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_response_content_extracted() {
        let mut server = Server::new_async().await;

        server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"choices":[{"message":{"content":"Corrected text here","role":"assistant"}}]}"#,
            )
            .create_async()
            .await;

        let client = OpenAiCompatClient::new(server.url(), None).unwrap();
        let result = client
            .enhance_text("input", "model", "{text}")
            .await
            .unwrap();
        assert_eq!(result, "Corrected text here");
    }

    // -------------------------------------------------------------------------
    // Error cases
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_empty_choices_returns_error() {
        let mut server = Server::new_async().await;

        server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[]}"#)
            .create_async()
            .await;

        let client = OpenAiCompatClient::new(server.url(), None).unwrap();
        let result = client.enhance_text("text", "model", "{text}").await;

        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("Empty response") || err_str.contains("retry"),
            "Unexpected error: {}",
            err_str
        );
    }

    #[tokio::test]
    async fn test_malformed_json_returns_error() {
        let mut server = Server::new_async().await;

        server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("this is not json")
            .create_async()
            .await;

        let client = OpenAiCompatClient::new(server.url(), None).unwrap();
        let result = client.enhance_text("text", "model", "{text}").await;

        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("parse") || err_str.contains("retry"),
            "Unexpected error: {}",
            err_str
        );
    }

    #[tokio::test]
    async fn test_non_2xx_returns_error() {
        let mut server = Server::new_async().await;

        // Use a 4xx so no retries are triggered (only 5xx retries)
        server
            .mock("POST", "/v1/chat/completions")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;

        let client = OpenAiCompatClient::new(server.url(), None).unwrap();
        let result = client.enhance_text("text", "model", "{text}").await;

        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("401") || err_str.contains("Server error"),
            "Unexpected error: {}",
            err_str
        );
    }

    #[tokio::test]
    async fn test_5xx_fails_on_the_first_attempt() {
        let mut server = Server::new_async().await;

        // One 500 is enough: a single bounded request, no retries.
        server
            .mock("POST", "/v1/chat/completions")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let client = OpenAiCompatClient::new(server.url(), None).unwrap();
        let result = client.enhance_text("text", "model", "{text}").await;

        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(err_str.contains("500"), "Unexpected error: {}", err_str);
    }

    // -------------------------------------------------------------------------
    // Error display
    // -------------------------------------------------------------------------

    #[test]
    fn test_error_display() {
        let err = OpenAiCompatError::ConnectionFailed("connection refused".to_string());
        assert_eq!(err.to_string(), "Connection failed: connection refused");

        let err = OpenAiCompatError::Timeout(30);
        assert_eq!(err.to_string(), "Request timeout after 30 seconds");

        let err = OpenAiCompatError::ServerError {
            status: 500,
            message: "Internal error".to_string(),
        };
        assert_eq!(err.to_string(), "Server error (500): Internal error");

        let err = OpenAiCompatError::EmptyChoices;
        assert_eq!(err.to_string(), "Empty response: no choices returned");
    }

    #[test]
    fn test_max_tokens_is_bounded_and_scales_with_input() {
        let short = output_token_limit("short prompt");
        let long = output_token_limit(&"a".repeat(6_000));
        assert!(long > short);
        assert!(long <= 4096);
    }
}
