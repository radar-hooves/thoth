//! Prompt template system for AI enhancement
//!
//! Provides built-in and custom prompt templates for text enhancement.
//! Custom prompts are stored in `~/.thoth/prompts.json`.

use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// A prompt template for AI enhancement
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTemplate {
    /// Unique identifier for the prompt
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// The prompt template with `{text}` placeholder
    pub template: String,
    /// Whether this is a built-in prompt (cannot be deleted)
    pub is_builtin: bool,
}

/// Get the built-in prompt templates
pub fn get_builtin_prompts() -> Vec<PromptTemplate> {
    vec![
        PromptTemplate {
            id: "fix-grammar".to_string(),
            name: "Fix Grammar".to_string(),
            template: "Fix any grammar and spelling mistakes in the following text. Keep the original meaning, tone, and length. Do not add extra content or explanations. Only output the corrected text:\n\n{text}".to_string(),
            is_builtin: true,
        },
        PromptTemplate {
            id: "make-professional".to_string(),
            name: "Make Professional".to_string(),
            template: "Rewrite the following text to be more professional and formal. Keep the same meaning and approximate length. Do not add extra content or explanations. Only output the rewritten text:\n\n{text}".to_string(),
            is_builtin: true,
        },
        PromptTemplate {
            id: "make-casual".to_string(),
            name: "Make Casual".to_string(),
            template: "Rewrite the following text to be more casual and conversational. Keep the same meaning and approximate length. Do not add extra content or explanations. Only output the rewritten text:\n\n{text}".to_string(),
            is_builtin: true,
        },
        PromptTemplate {
            id: "simplify".to_string(),
            name: "Simplify".to_string(),
            template: "Simplify the following text to be easier to understand. Use shorter sentences and simpler words. Keep the same meaning and approximate length. Do not add extra content or explanations. Only output the simplified text:\n\n{text}".to_string(),
            is_builtin: true,
        },
        PromptTemplate {
            id: "summarise".to_string(),
            name: "Summarise".to_string(),
            template: "Summarise the following text concisely in 1-2 sentences. Keep only the most important points. Only output the summary:\n\n{text}".to_string(),
            is_builtin: true,
        },
        PromptTemplate {
            id: "expand".to_string(),
            name: "Expand".to_string(),
            template: "Expand the following text with 2-3x more detail and explanation. Keep the same style and tone. Only output the expanded text:\n\n{text}".to_string(),
            is_builtin: true,
        },
        PromptTemplate {
            id: "pirate-speak".to_string(),
            name: "Speak Like a Pirate".to_string(),
            template: "Rewrite the following text in pirate dialect. Use pirate vocabulary and speech patterns. Keep the same meaning and approximate length. Do not add extra content or explanations. Only output the rewritten text:\n\n{text}".to_string(),
            is_builtin: true,
        },
        PromptTemplate {
            id: "ai-dev".to_string(),
            name: "AI / Dev (Compressed)".to_string(),
            // Strips dictation noise for token-efficient input to AI coding tools.
            template: "Remove all filler words (um, uh, like, you know, basically, sort of, kind of), hedging phrases (I think maybe, I guess, perhaps we could), meta-commentary (let me think, what I'm trying to say is, does that make sense), and rhetorical questions addressed to the listener. Collapse rambling into direct, declarative statements. Preserve every technical fact, name, number, identifier, and the actual request or intent — do not summarise away content, only remove noise. Output only the cleaned text with no preamble, no explanation, and no quotes:\n\n{text}".to_string(),
            is_builtin: true,
        },
    ]
}

/// Get the path to custom prompts file
pub fn get_custom_prompts_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".thoth")
        .join("prompts.json")
}

/// Load custom prompts from disk
pub fn load_custom_prompts(path: &PathBuf) -> Vec<PromptTemplate> {
    if !path.exists() {
        return Vec::new();
    }

    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            tracing::warn!("Failed to parse custom prompts: {}", e);
            Vec::new()
        }),
        Err(e) => {
            tracing::warn!("Failed to read custom prompts file: {}", e);
            Vec::new()
        }
    }
}

/// Save a custom prompt to disk
pub fn save_custom_prompt(path: &PathBuf, prompt: &PromptTemplate) -> Result<(), String> {
    // Ensure directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    // Load existing prompts
    let mut prompts = load_custom_prompts(path);

    // Update or add the prompt
    if let Some(existing) = prompts.iter_mut().find(|p| p.id == prompt.id) {
        *existing = prompt.clone();
    } else {
        prompts.push(prompt.clone());
    }

    // Save back
    let content = serde_json::to_string_pretty(&prompts)
        .map_err(|e| format!("Failed to serialise: {}", e))?;

    fs::write(path, content).map_err(|e| format!("Failed to write prompts file: {}", e))?;

    tracing::info!("Saved custom prompt: {}", prompt.id);
    Ok(())
}

/// Delete a custom prompt
pub fn delete_custom_prompt(path: &PathBuf, prompt_id: &str) -> Result<(), String> {
    let mut prompts = load_custom_prompts(path);
    let original_len = prompts.len();

    prompts.retain(|p| p.id != prompt_id);

    if prompts.len() == original_len {
        return Err(format!("Prompt '{}' not found", prompt_id));
    }

    let content = serde_json::to_string_pretty(&prompts)
        .map_err(|e| format!("Failed to serialise: {}", e))?;

    fs::write(path, content).map_err(|e| format!("Failed to write prompts file: {}", e))?;

    tracing::info!("Deleted custom prompt: {}", prompt_id);
    Ok(())
}

/// Apply a prompt template to text
pub fn apply_prompt(template: &PromptTemplate, text: &str) -> String {
    template.template.replace("{text}", text)
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// Get all prompt templates (built-in and custom)
#[tauri::command]
pub fn get_all_prompts() -> Vec<PromptTemplate> {
    tauri_plugin_telemetry::traced_sync("get_all_prompts", || {
        Ok::<_, std::convert::Infallible>({
            let mut prompts = get_builtin_prompts();
            let custom_path = get_custom_prompts_path();
            let custom_prompts = load_custom_prompts(&custom_path);
            prompts.extend(custom_prompts);
            prompts
        })
    })
    .unwrap()
}

/// Get only built-in prompt templates
#[tauri::command]
pub fn get_builtin_prompts_cmd() -> Vec<PromptTemplate> {
    tauri_plugin_telemetry::traced_sync("get_builtin_prompts_cmd", || {
        Ok::<_, std::convert::Infallible>(get_builtin_prompts())
    })
    .unwrap()
}

/// Get only custom prompt templates
#[tauri::command]
pub fn get_custom_prompts_cmd() -> Vec<PromptTemplate> {
    tauri_plugin_telemetry::traced_sync("get_custom_prompts_cmd", || {
        Ok::<_, std::convert::Infallible>({
            let custom_path = get_custom_prompts_path();
            load_custom_prompts(&custom_path)
        })
    })
    .unwrap()
}

/// Add or update a custom prompt template
#[tauri::command]
pub async fn save_custom_prompt_cmd(prompt: PromptTemplate) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("save_custom_prompt_cmd", async move {
        if prompt.is_builtin {
            return Err("Cannot save a built-in prompt as custom".to_string().into());
        }

        if prompt.id.is_empty() {
            return Err("Prompt ID cannot be empty".to_string().into());
        }

        if prompt.name.is_empty() {
            return Err("Prompt name cannot be empty".to_string().into());
        }

        if prompt.template.is_empty() {
            return Err("Prompt template cannot be empty".to_string().into());
        }

        if !prompt.template.contains("{text}") {
            return Err("Prompt template must contain {text} placeholder"
                .to_string()
                .into());
        }

        let custom_path = get_custom_prompts_path();
        save_custom_prompt(&custom_path, &prompt).map_err(Into::into)
    })
    .await
}

/// Delete a custom prompt template
#[tauri::command]
pub async fn delete_custom_prompt_cmd(prompt_id: String) -> Result<(), Error> {
    tauri_plugin_telemetry::traced("delete_custom_prompt_cmd", async move {
        // Check if it's a built-in prompt
        if get_builtin_prompts().iter().any(|p| p.id == prompt_id) {
            return Err("Cannot delete a built-in prompt".to_string().into());
        }

        let custom_path = get_custom_prompts_path();
        delete_custom_prompt(&custom_path, &prompt_id).map_err(Into::into)
    })
    .await
}

/// Get a prompt by ID
#[tauri::command]
pub fn get_prompt_by_id(prompt_id: String) -> Option<PromptTemplate> {
    tauri_plugin_telemetry::traced_sync("get_prompt_by_id", || {
        Ok::<_, std::convert::Infallible>({
            let all_prompts = get_all_prompts();
            all_prompts.into_iter().find(|p| p.id == prompt_id)
        })
    })
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // =========================================================================
    // Built-in prompts tests
    // =========================================================================

    #[test]
    fn test_builtin_prompts_exist() {
        let prompts = get_builtin_prompts();
        assert!(!prompts.is_empty());
        assert!(prompts.iter().all(|p| p.is_builtin));
    }

    #[test]
    fn test_builtin_prompts_have_required_fields() {
        let prompts = get_builtin_prompts();

        for prompt in prompts {
            assert!(!prompt.id.is_empty(), "Prompt ID should not be empty");
            assert!(!prompt.name.is_empty(), "Prompt name should not be empty");
            assert!(
                !prompt.template.is_empty(),
                "Prompt template should not be empty"
            );
            assert!(
                prompt.template.contains("{text}"),
                "Prompt template should contain {{text}} placeholder"
            );
            assert!(
                prompt.is_builtin,
                "Built-in prompt should have is_builtin=true"
            );
        }
    }

    #[test]
    fn test_builtin_prompts_have_unique_ids() {
        let prompts = get_builtin_prompts();
        let ids: Vec<&str> = prompts.iter().map(|p| p.id.as_str()).collect();

        for (i, id) in ids.iter().enumerate() {
            assert!(
                !ids[i + 1..].contains(id),
                "Duplicate prompt ID found: {}",
                id
            );
        }
    }

    #[test]
    fn test_builtin_prompts_include_expected() {
        let prompts = get_builtin_prompts();
        let ids: Vec<&str> = prompts.iter().map(|p| p.id.as_str()).collect();

        assert!(
            ids.contains(&"fix-grammar"),
            "Should have fix-grammar prompt"
        );
        assert!(
            ids.contains(&"make-professional"),
            "Should have make-professional prompt"
        );
        assert!(
            ids.contains(&"make-casual"),
            "Should have make-casual prompt"
        );
        assert!(ids.contains(&"simplify"), "Should have simplify prompt");
        assert!(ids.contains(&"summarise"), "Should have summarise prompt");
        assert!(ids.contains(&"expand"), "Should have expand prompt");
        assert!(
            ids.contains(&"pirate-speak"),
            "Should have pirate-speak prompt"
        );
        assert!(ids.contains(&"ai-dev"), "Should have ai-dev prompt");
    }

    // =========================================================================
    // Apply prompt tests
    // =========================================================================

    #[test]
    fn test_apply_prompt() {
        let template = PromptTemplate {
            id: "test".to_string(),
            name: "Test".to_string(),
            template: "Process this: {text}".to_string(),
            is_builtin: false,
        };

        let result = apply_prompt(&template, "hello world");
        assert_eq!(result, "Process this: hello world");
    }

    #[test]
    fn test_apply_prompt_empty_text() {
        let template = PromptTemplate {
            id: "test".to_string(),
            name: "Test".to_string(),
            template: "Process this: {text}".to_string(),
            is_builtin: false,
        };

        let result = apply_prompt(&template, "");
        assert_eq!(result, "Process this: ");
    }

    #[test]
    fn test_apply_prompt_with_newlines() {
        let template = PromptTemplate {
            id: "test".to_string(),
            name: "Test".to_string(),
            template: "Fix this:\n{text}\nEnd.".to_string(),
            is_builtin: false,
        };

        let result = apply_prompt(&template, "line1\nline2");
        assert_eq!(result, "Fix this:\nline1\nline2\nEnd.");
    }

    #[test]
    fn test_apply_prompt_preserves_special_chars() {
        let template = PromptTemplate {
            id: "test".to_string(),
            name: "Test".to_string(),
            template: "Input: {text}".to_string(),
            is_builtin: false,
        };

        let result = apply_prompt(&template, "Hello \"world\" <test> & more");
        assert_eq!(result, "Input: Hello \"world\" <test> & more");
    }

    #[test]
    fn test_apply_prompt_only_replaces_placeholder() {
        let template = PromptTemplate {
            id: "test".to_string(),
            name: "Test".to_string(),
            template: "{text} is the {text}".to_string(),
            is_builtin: false,
        };

        let result = apply_prompt(&template, "input");
        // Note: replace replaces ALL occurrences
        assert_eq!(result, "input is the input");
    }

    // =========================================================================
    // PromptTemplate struct tests
    // =========================================================================

    #[test]
    fn test_prompt_template_serialisation() {
        let prompt = PromptTemplate {
            id: "test-id".to_string(),
            name: "Test Name".to_string(),
            template: "Hello {text}".to_string(),
            is_builtin: false,
        };

        let json = serde_json::to_string(&prompt).unwrap();

        // Check camelCase field names
        assert!(json.contains("\"isBuiltin\":false"));

        let restored: PromptTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, prompt.id);
        assert_eq!(restored.name, prompt.name);
    }

    #[test]
    fn test_prompt_template_deserialisation_ignores_unknown_fields() {
        // Existing prompts.json files may still contain triggerWords; serde should ignore them
        let json = r#"{"id":"test","name":"Test","template":"{text}","isBuiltin":false,"triggerWords":["hello"]}"#;
        let prompt: PromptTemplate = serde_json::from_str(json).unwrap();

        assert_eq!(prompt.id, "test");
    }

    #[test]
    fn test_prompt_template_clone() {
        let original = PromptTemplate {
            id: "original".to_string(),
            name: "Original".to_string(),
            template: "{text}".to_string(),
            is_builtin: true,
        };

        let cloned = original.clone();

        assert_eq!(cloned.id, original.id);
        assert_eq!(cloned.name, original.name);
        assert_eq!(cloned.template, original.template);
        assert_eq!(cloned.is_builtin, original.is_builtin);
    }

    // =========================================================================
    // Custom prompts path test
    // =========================================================================

    #[test]
    fn test_custom_prompts_path_format() {
        let path = get_custom_prompts_path();
        let path_str = path.to_string_lossy();

        assert!(path_str.contains(".thoth"));
        assert!(path_str.ends_with("prompts.json"));
    }

    // =========================================================================
    // Load/Save custom prompts tests
    // =========================================================================

    #[test]
    fn test_load_custom_prompts_nonexistent_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("nonexistent.json");

        let prompts = load_custom_prompts(&path);
        assert!(prompts.is_empty());
    }

    #[test]
    fn test_load_custom_prompts_invalid_json() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("invalid.json");

        std::fs::write(&path, "not valid json").unwrap();

        let prompts = load_custom_prompts(&path);
        assert!(prompts.is_empty());
    }

    #[test]
    fn test_load_custom_prompts_empty_array() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("empty.json");

        std::fs::write(&path, "[]").unwrap();

        let prompts = load_custom_prompts(&path);
        assert!(prompts.is_empty());
    }

    #[test]
    fn test_load_custom_prompts_valid() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("prompts.json");

        let json = r#"[{"id":"custom1","name":"Custom 1","template":"{text}","isBuiltin":false}]"#;
        std::fs::write(&path, json).unwrap();

        let prompts = load_custom_prompts(&path);
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].id, "custom1");
    }

    #[test]
    fn test_save_custom_prompt_creates_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("new_prompts.json");

        let prompt = PromptTemplate {
            id: "new".to_string(),
            name: "New Prompt".to_string(),
            template: "Process: {text}".to_string(),
            is_builtin: false,
        };

        save_custom_prompt(&path, &prompt).expect("Save should succeed");

        assert!(path.exists());
        let prompts = load_custom_prompts(&path);
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].id, "new");
    }

    #[test]
    fn test_save_custom_prompt_updates_existing() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("prompts.json");

        let prompt1 = PromptTemplate {
            id: "test".to_string(),
            name: "Original".to_string(),
            template: "{text}".to_string(),
            is_builtin: false,
        };

        save_custom_prompt(&path, &prompt1).unwrap();

        let prompt2 = PromptTemplate {
            id: "test".to_string(),
            name: "Updated".to_string(),
            template: "New: {text}".to_string(),
            is_builtin: false,
        };

        save_custom_prompt(&path, &prompt2).unwrap();

        let prompts = load_custom_prompts(&path);
        assert_eq!(prompts.len(), 1); // Should update, not add
        assert_eq!(prompts[0].name, "Updated");
        assert_eq!(prompts[0].template, "New: {text}");
    }

    #[test]
    fn test_save_custom_prompt_adds_new() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("prompts.json");

        let prompt1 = PromptTemplate {
            id: "first".to_string(),
            name: "First".to_string(),
            template: "{text}".to_string(),
            is_builtin: false,
        };

        save_custom_prompt(&path, &prompt1).unwrap();

        let prompt2 = PromptTemplate {
            id: "second".to_string(),
            name: "Second".to_string(),
            template: "{text}".to_string(),
            is_builtin: false,
        };

        save_custom_prompt(&path, &prompt2).unwrap();

        let prompts = load_custom_prompts(&path);
        assert_eq!(prompts.len(), 2);
    }

    // =========================================================================
    // Delete custom prompt tests
    // =========================================================================

    #[test]
    fn test_delete_custom_prompt_success() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("prompts.json");

        // Create two prompts
        let prompt1 = PromptTemplate {
            id: "keep".to_string(),
            name: "Keep".to_string(),
            template: "{text}".to_string(),
            is_builtin: false,
        };
        let prompt2 = PromptTemplate {
            id: "delete".to_string(),
            name: "Delete".to_string(),
            template: "{text}".to_string(),
            is_builtin: false,
        };

        save_custom_prompt(&path, &prompt1).unwrap();
        save_custom_prompt(&path, &prompt2).unwrap();

        delete_custom_prompt(&path, "delete").expect("Delete should succeed");

        let prompts = load_custom_prompts(&path);
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].id, "keep");
    }

    #[test]
    fn test_delete_custom_prompt_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("prompts.json");

        let prompt = PromptTemplate {
            id: "existing".to_string(),
            name: "Existing".to_string(),
            template: "{text}".to_string(),
            is_builtin: false,
        };

        save_custom_prompt(&path, &prompt).unwrap();

        let result = delete_custom_prompt(&path, "nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }
}
