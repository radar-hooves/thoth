//! Export and search functionality for transcription history.
//!
//! Provides commands for searching transcriptions and exporting them
//! to various formats (JSON, CSV, TXT).

use crate::database;
use crate::error::Error;
use chrono::{DateTime, Utc};
use csv::WriterBuilder;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// A transcription record for export and search operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionRecord {
    pub id: String,
    pub text: String,
    pub raw_text: Option<String>,
    pub duration_seconds: Option<f64>,
    pub created_at: String,
    pub audio_path: Option<String>,
    pub is_enhanced: bool,
    pub enhancement_prompt: Option<String>,
    pub transcription_model_name: Option<String>,
    pub transcription_duration_seconds: Option<f64>,
    pub enhancement_model_name: Option<String>,
    pub enhancement_duration_seconds: Option<f64>,
}

/// Export format options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Json,
    Csv,
    Txt,
}

/// Search parameters for filtering transcriptions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParams {
    pub query: Option<String>,
    pub from_date: Option<i64>,
    pub to_date: Option<i64>,
    pub enhanced_only: Option<bool>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// Search result containing records and pagination info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub records: Vec<TranscriptionRecord>,
    pub total_count: u32,
    pub has_more: bool,
}

// =============================================================================
// Database Operations
// =============================================================================

/// Searches transcriptions in the database with full-text search and date filtering.
fn search_transcriptions_db(params: &SearchParams) -> Result<SearchResult, String> {
    let conn = database::open_connection().map_err(|e| {
        tracing::error!("Failed to open database connection: {}", e);
        format!("Failed to open database: {}", e)
    })?;

    // Build the query dynamically based on parameters
    let mut where_clauses: Vec<String> = Vec::new();
    let mut query_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    // Full-text search on text and raw_text
    if let Some(query) = &params.query {
        if !query.trim().is_empty() {
            // Case-insensitive search using LIKE
            let search_pattern = format!("%{}%", query.trim());
            where_clauses.push(
                "(text LIKE ?1 COLLATE NOCASE OR raw_text LIKE ?1 COLLATE NOCASE)".to_string(),
            );
            query_params.push(Box::new(search_pattern));
        }
    }

    // Date range filtering
    if let Some(from_timestamp) = params.from_date {
        let from_date = DateTime::<Utc>::from_timestamp(from_timestamp, 0)
            .ok_or("Invalid from_date timestamp")?;
        let from_str = from_date.format("%Y-%m-%dT%H:%M:%S").to_string();
        let param_idx = query_params.len() + 1;
        where_clauses.push(format!("created_at >= ?{}", param_idx));
        query_params.push(Box::new(from_str));
    }

    if let Some(to_timestamp) = params.to_date {
        let to_date =
            DateTime::<Utc>::from_timestamp(to_timestamp, 0).ok_or("Invalid to_date timestamp")?;
        let to_str = to_date.format("%Y-%m-%dT%H:%M:%S").to_string();
        let param_idx = query_params.len() + 1;
        where_clauses.push(format!("created_at <= ?{}", param_idx));
        query_params.push(Box::new(to_str));
    }

    // Enhanced filter
    if let Some(enhanced_only) = params.enhanced_only {
        if enhanced_only {
            let param_idx = query_params.len() + 1;
            where_clauses.push(format!("is_enhanced = ?{}", param_idx));
            query_params.push(Box::new(1i32));
        }
    }

    // Build the WHERE clause
    let where_clause = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    // Get total count first
    let count_sql = format!("SELECT COUNT(*) FROM transcriptions {}", where_clause);
    let total_count: u32 = {
        let mut stmt = conn.prepare(&count_sql).map_err(|e| e.to_string())?;
        let params_refs: Vec<&dyn rusqlite::ToSql> =
            query_params.iter().map(|p| p.as_ref()).collect();
        stmt.query_row(params_refs.as_slice(), |row| row.get(0))
            .map_err(|e| e.to_string())?
    };

    // Build the main query with pagination
    let limit = params.limit.unwrap_or(100);
    let offset = params.offset.unwrap_or(0);

    let sql = format!(
        r#"
        SELECT id, text, raw_text, duration_seconds, created_at,
               audio_path, is_enhanced, enhancement_prompt,
               transcription_model_name, transcription_duration_seconds,
               enhancement_model_name, enhancement_duration_seconds
        FROM transcriptions
        {}
        ORDER BY created_at DESC
        LIMIT {} OFFSET {}
        "#,
        where_clause, limit, offset
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let params_refs: Vec<&dyn rusqlite::ToSql> = query_params.iter().map(|p| p.as_ref()).collect();

    let records = stmt
        .query_map(params_refs.as_slice(), export_row_to_record)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let has_more = offset + (records.len() as u32) < total_count;

    Ok(SearchResult {
        records,
        total_count,
        has_more,
    })
}

/// Map a database row to an export TranscriptionRecord.
fn export_row_to_record(row: &rusqlite::Row) -> rusqlite::Result<TranscriptionRecord> {
    Ok(TranscriptionRecord {
        id: row.get(0)?,
        text: row.get(1)?,
        raw_text: row.get(2)?,
        duration_seconds: row.get(3)?,
        created_at: row.get(4)?,
        audio_path: row.get(5)?,
        is_enhanced: row.get::<_, i32>(6)? != 0,
        enhancement_prompt: row.get(7)?,
        transcription_model_name: row.get(8)?,
        transcription_duration_seconds: row.get(9)?,
        enhancement_model_name: row.get(10)?,
        enhancement_duration_seconds: row.get(11)?,
    })
}

/// Gets transcriptions by their IDs for batch export.
fn get_transcriptions_by_ids(ids: &[String]) -> Result<Vec<TranscriptionRecord>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let conn = database::open_connection().map_err(|e| {
        tracing::error!("Failed to open database connection: {}", e);
        format!("Failed to open database: {}", e)
    })?;

    // Build IN clause with placeholders
    let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{}", i)).collect();
    let sql = format!(
        r#"
        SELECT id, text, raw_text, duration_seconds, created_at,
               audio_path, is_enhanced, enhancement_prompt,
               transcription_model_name, transcription_duration_seconds,
               enhancement_model_name, enhancement_duration_seconds
        FROM transcriptions
        WHERE id IN ({})
        ORDER BY created_at DESC
        "#,
        placeholders.join(", ")
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();

    let records = stmt
        .query_map(params.as_slice(), export_row_to_record)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(records)
}

// =============================================================================
// Export Functions
// =============================================================================

/// Exports records to JSON format.
fn export_json(records: &[TranscriptionRecord], path: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(records).map_err(|e| {
        tracing::error!("Failed to serialise records to JSON: {}", e);
        format!("Failed to create JSON: {}", e)
    })?;

    let mut file = File::create(path).map_err(|e| {
        tracing::error!("Failed to create export file: {}", e);
        format!("Failed to create file: {}", e)
    })?;

    file.write_all(json.as_bytes()).map_err(|e| {
        tracing::error!("Failed to write JSON to file: {}", e);
        format!("Failed to write file: {}", e)
    })?;

    tracing::info!("Exported {} records to JSON: {:?}", records.len(), path);
    Ok(())
}

/// Exports records to CSV format.
///
/// Uses the `csv` crate for RFC-4180-compliant quoting. Free-text string fields
/// (transcription text, raw text, model names, enhancement prompt) are
/// formula-injection sanitised before writing; numeric, boolean, and timestamp
/// columns are written verbatim.
fn export_csv(records: &[TranscriptionRecord], path: &Path) -> Result<(), String> {
    let file = File::create(path).map_err(|e| {
        tracing::error!("Failed to create export file: {}", e);
        format!("Failed to create file: {}", e)
    })?;

    let mut wtr = WriterBuilder::new().from_writer(file);

    // Header — exact column names preserved for downstream compatibility.
    wtr.write_record([
        "id",
        "text",
        "raw_text",
        "duration_seconds",
        "created_at",
        "audio_path",
        "is_enhanced",
        "enhancement_prompt",
        "transcription_model_name",
        "transcription_duration_seconds",
        "enhancement_model_name",
        "enhancement_duration_seconds",
    ])
    .map_err(|e| format!("Failed to write CSV header: {}", e))?;

    for record in records {
        wtr.write_record([
            // Identifier and path fields: not free-text, no injection guard needed.
            record.id.as_str(),
            // Free-text fields dictated by the user: apply injection guard.
            &sanitize_csv_field(&record.text),
            &sanitize_csv_field(record.raw_text.as_deref().unwrap_or("")),
            // Numeric column: written as a plain string so empty stays empty.
            &record
                .duration_seconds
                .map_or_else(String::new, |d| d.to_string()),
            // Timestamp: structured, not user-controlled.
            record.created_at.as_str(),
            // Filesystem path: structured, not user-controlled.
            record.audio_path.as_deref().unwrap_or(""),
            // Boolean: always "true"/"false", cannot be a formula.
            &record.is_enhanced.to_string(),
            // Free-text fields.
            &sanitize_csv_field(record.enhancement_prompt.as_deref().unwrap_or("")),
            &sanitize_csv_field(record.transcription_model_name.as_deref().unwrap_or("")),
            // Numeric columns.
            &record
                .transcription_duration_seconds
                .map_or_else(String::new, |d| d.to_string()),
            // Free-text field.
            &sanitize_csv_field(record.enhancement_model_name.as_deref().unwrap_or("")),
            // Numeric column.
            &record
                .enhancement_duration_seconds
                .map_or_else(String::new, |d| d.to_string()),
        ])
        .map_err(|e| format!("Failed to write CSV row: {}", e))?;
    }

    wtr.flush()
        .map_err(|e| format!("Failed to flush CSV writer: {}", e))?;

    tracing::info!("Exported {} records to CSV: {:?}", records.len(), path);
    Ok(())
}

/// Prefixes a string cell with a single apostrophe if its first character is a
/// spreadsheet formula trigger (`=`, `+`, `-`, `@`, tab, or carriage-return).
///
/// This is the standard neutralisation technique (CWE-1236) that causes
/// Excel, Google Sheets, and LibreOffice Calc to treat the value as text
/// rather than evaluating it as a formula. Applied only to free-text fields
/// that users dictate; numeric and structured fields are left untouched.
pub(crate) fn sanitize_csv_field(s: &str) -> String {
    match s.chars().next() {
        Some('=' | '+' | '-' | '@') | Some('\t') | Some('\r') => format!("'{}", s),
        _ => s.to_string(),
    }
}

/// Exports records to plain text format.
fn export_txt(records: &[TranscriptionRecord], path: &Path) -> Result<(), String> {
    let mut file = File::create(path).map_err(|e| {
        tracing::error!("Failed to create export file: {}", e);
        format!("Failed to create file: {}", e)
    })?;

    for (i, record) in records.iter().enumerate() {
        if i > 0 {
            writeln!(file, "\n{}", "-".repeat(80))
                .map_err(|e| format!("Failed to write: {}", e))?;
        }

        writeln!(file, "Date: {}", record.created_at)
            .map_err(|e| format!("Failed to write: {}", e))?;

        if let Some(duration) = record.duration_seconds {
            writeln!(file, "Duration: {:.1}s", duration)
                .map_err(|e| format!("Failed to write: {}", e))?;
        }

        if record.is_enhanced {
            writeln!(file, "Enhanced: Yes").map_err(|e| format!("Failed to write: {}", e))?;
        }

        writeln!(file).map_err(|e| format!("Failed to write: {}", e))?;
        writeln!(file, "{}", record.text).map_err(|e| format!("Failed to write: {}", e))?;
    }

    tracing::info!("Exported {} records to TXT: {:?}", records.len(), path);
    Ok(())
}

// =============================================================================
// Tauri Commands
// =============================================================================

/// Searches transcription history with full-text search and date filtering.
///
/// # Arguments
/// * `query` - Optional search text (searches in text and raw_text fields)
/// * `from_date` - Optional Unix timestamp for start of date range
/// * `to_date` - Optional Unix timestamp for end of date range
/// * `enhanced_only` - If true, only return enhanced transcriptions
/// * `limit` - Maximum number of records to return (default: 100)
/// * `offset` - Number of records to skip for pagination (default: 0)
#[tauri::command]
pub fn search_history(
    query: Option<String>,
    from_date: Option<i64>,
    to_date: Option<i64>,
    enhanced_only: Option<bool>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<SearchResult, Error> {
    let params = SearchParams {
        query,
        from_date,
        to_date,
        enhanced_only,
        limit,
        offset,
    };

    tracing::debug!("Searching history with params: {:?}", params);
    search_transcriptions_db(&params).map_err(Into::into)
}

/// Generic export function that handles record fetching and calls the format-specific exporter.
fn export_records<F>(
    ids: &[String],
    path: &Path,
    search_params: Option<SearchParams>,
    exporter: F,
) -> Result<u32, String>
where
    F: FnOnce(&[TranscriptionRecord], &Path) -> Result<(), String>,
{
    let records = if ids.is_empty() {
        let params = search_params.unwrap_or(SearchParams {
            query: None,
            from_date: None,
            to_date: None,
            enhanced_only: None,
            limit: Some(10000),
            offset: Some(0),
        });
        search_transcriptions_db(&params)?.records
    } else {
        get_transcriptions_by_ids(ids)?
    };

    let count = records.len() as u32;
    exporter(&records, path)?;
    Ok(count)
}

/// Exports transcription records to a JSON file.
#[tauri::command]
pub async fn export_to_json(
    ids: Vec<String>,
    path: String,
    search_params: Option<SearchParams>,
) -> Result<u32, Error> {
    tauri_plugin_telemetry::traced("export_to_json", async move {
        export_records(&ids, Path::new(&path), search_params, export_json).map_err(Into::into)
    })
    .await
}

/// Exports transcription records to a CSV file.
#[tauri::command]
pub async fn export_to_csv(
    ids: Vec<String>,
    path: String,
    search_params: Option<SearchParams>,
) -> Result<u32, Error> {
    tauri_plugin_telemetry::traced("export_to_csv", async move {
        export_records(&ids, Path::new(&path), search_params, export_csv).map_err(Into::into)
    })
    .await
}

/// Exports transcription records to a plain text file.
#[tauri::command]
pub async fn export_to_txt(
    ids: Vec<String>,
    path: String,
    search_params: Option<SearchParams>,
) -> Result<u32, Error> {
    tauri_plugin_telemetry::traced("export_to_txt", async move {
        export_records(&ids, Path::new(&path), search_params, export_txt).map_err(Into::into)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // =========================================================================
    // sanitize_csv_field tests
    // =========================================================================

    #[test]
    fn test_sanitize_csv_field_plain_text_unchanged() {
        assert_eq!(sanitize_csv_field("hello world"), "hello world");
    }

    #[test]
    fn test_sanitize_csv_field_empty_unchanged() {
        assert_eq!(sanitize_csv_field(""), "");
    }

    #[test]
    fn test_sanitize_csv_field_leading_equals_prefixed() {
        // Formula-injection guard: = triggers an apostrophe prefix.
        assert_eq!(sanitize_csv_field("=SUM(A1:A2)"), "'=SUM(A1:A2)");
    }

    #[test]
    fn test_sanitize_csv_field_leading_plus_prefixed() {
        assert_eq!(sanitize_csv_field("+1300"), "'+1300");
    }

    #[test]
    fn test_sanitize_csv_field_leading_minus_prefixed() {
        assert_eq!(sanitize_csv_field("-CMD"), "'-CMD");
    }

    #[test]
    fn test_sanitize_csv_field_leading_at_prefixed() {
        assert_eq!(sanitize_csv_field("@SUM"), "'@SUM");
    }

    #[test]
    fn test_sanitize_csv_field_leading_tab_prefixed() {
        assert_eq!(sanitize_csv_field("\tSUM"), "'\tSUM");
    }

    #[test]
    fn test_sanitize_csv_field_leading_cr_prefixed() {
        assert_eq!(sanitize_csv_field("\rSUM"), "'\rSUM");
    }

    #[test]
    fn test_sanitize_csv_field_equals_not_at_start_unchanged() {
        // Only the first character is checked; mid-string = is fine.
        assert_eq!(sanitize_csv_field("a=b"), "a=b");
    }

    #[test]
    fn test_sanitize_csv_field_whitespace_unchanged() {
        assert_eq!(sanitize_csv_field("  hello  "), "  hello  ");
    }

    // =========================================================================
    // Formula-injection integration tests (full CSV round-trip)
    // =========================================================================

    /// Parses the CSV content produced by export_csv and returns a Vec of rows
    /// (each row is a Vec of String fields), skipping the header row.
    fn parse_csv_rows(content: &str) -> Vec<Vec<String>> {
        let mut rdr = csv::Reader::from_reader(content.as_bytes());
        rdr.records()
            .map(|r| r.unwrap().iter().map(|f| f.to_string()).collect())
            .collect()
    }

    #[test]
    fn test_export_csv_formula_injection_neutralised() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("injection.csv");

        let records = vec![TranscriptionRecord {
            id: "id3".to_string(),
            text: "=SUM(A1:A2)".to_string(),
            raw_text: Some("+malicious".to_string()),
            duration_seconds: Some(2.0),
            created_at: "2024-01-15T10:40:00".to_string(),
            audio_path: None,
            is_enhanced: false,
            enhancement_prompt: Some("@BAD".to_string()),
            transcription_model_name: Some("-model".to_string()),
            transcription_duration_seconds: None,
            enhancement_model_name: None,
            enhancement_duration_seconds: None,
        }];

        export_csv(&records, &path).expect("Export should succeed");

        let content = std::fs::read_to_string(&path).unwrap();
        let rows = parse_csv_rows(&content);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];

        // text column (index 1): formula must be prefixed.
        assert_eq!(row[1], "'=SUM(A1:A2)");
        // raw_text column (index 2): + prefix must be neutralised.
        assert_eq!(row[2], "'+malicious");
        // enhancement_prompt column (index 7): @ prefix must be neutralised.
        assert_eq!(row[7], "'@BAD");
        // transcription_model_name column (index 8): - prefix must be neutralised.
        assert_eq!(row[8], "'-model");
    }

    #[test]
    fn test_export_csv_numeric_column_not_prefixed() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("numeric.csv");

        let records = vec![TranscriptionRecord {
            id: "id4".to_string(),
            text: "normal".to_string(),
            raw_text: None,
            duration_seconds: Some(5.5),
            created_at: "2024-01-15T10:45:00".to_string(),
            audio_path: None,
            is_enhanced: false,
            enhancement_prompt: None,
            transcription_model_name: None,
            transcription_duration_seconds: Some(1.0),
            enhancement_model_name: None,
            enhancement_duration_seconds: Some(0.5),
        }];

        export_csv(&records, &path).expect("Export should succeed");

        let content = std::fs::read_to_string(&path).unwrap();
        let rows = parse_csv_rows(&content);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];

        // duration_seconds (index 3): must be a plain number, not apostrophe-prefixed.
        assert_eq!(row[3], "5.5");
        // transcription_duration_seconds (index 9).
        assert_eq!(row[9], "1");
        // enhancement_duration_seconds (index 11).
        assert_eq!(row[11], "0.5");
    }

    // =========================================================================
    // TranscriptionRecord tests
    // =========================================================================

    #[test]
    fn test_transcription_record_creation() {
        let record = TranscriptionRecord {
            id: "test-id".to_string(),
            text: "Hello world".to_string(),
            raw_text: Some("hello world".to_string()),
            duration_seconds: Some(5.5),
            created_at: "2024-01-15T10:30:00".to_string(),
            audio_path: Some("/path/to/audio.wav".to_string()),
            is_enhanced: true,
            enhancement_prompt: Some("fix-grammar".to_string()),
            transcription_model_name: None,
            transcription_duration_seconds: None,
            enhancement_model_name: None,
            enhancement_duration_seconds: None,
        };

        assert_eq!(record.id, "test-id");
        assert_eq!(record.text, "Hello world");
        assert!(record.is_enhanced);
    }

    #[test]
    fn test_transcription_record_optional_fields() {
        let record = TranscriptionRecord {
            id: "test-id".to_string(),
            text: "Hello".to_string(),
            raw_text: None,
            duration_seconds: None,
            created_at: "2024-01-15T10:30:00".to_string(),
            audio_path: None,
            is_enhanced: false,
            enhancement_prompt: None,
            transcription_model_name: None,
            transcription_duration_seconds: None,
            enhancement_model_name: None,
            enhancement_duration_seconds: None,
        };

        assert!(record.raw_text.is_none());
        assert!(record.duration_seconds.is_none());
        assert!(record.audio_path.is_none());
        assert!(record.enhancement_prompt.is_none());
    }

    #[test]
    fn test_transcription_record_serialisation() {
        let record = TranscriptionRecord {
            id: "uuid-123".to_string(),
            text: "Test text".to_string(),
            raw_text: Some("test text".to_string()),
            duration_seconds: Some(2.5),
            created_at: "2024-01-15T10:30:00".to_string(),
            audio_path: None,
            is_enhanced: false,
            enhancement_prompt: None,
            transcription_model_name: None,
            transcription_duration_seconds: None,
            enhancement_model_name: None,
            enhancement_duration_seconds: None,
        };

        let json = serde_json::to_string(&record).unwrap();
        let restored: TranscriptionRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.id, record.id);
        assert_eq!(restored.text, record.text);
        assert_eq!(restored.duration_seconds, record.duration_seconds);
    }

    // =========================================================================
    // ExportFormat tests
    // =========================================================================

    #[test]
    fn test_export_format_json() {
        let format = ExportFormat::Json;
        assert_eq!(format, ExportFormat::Json);
    }

    #[test]
    fn test_export_format_csv() {
        let format = ExportFormat::Csv;
        assert_eq!(format, ExportFormat::Csv);
    }

    #[test]
    fn test_export_format_txt() {
        let format = ExportFormat::Txt;
        assert_eq!(format, ExportFormat::Txt);
    }

    #[test]
    fn test_export_format_serialisation() {
        assert_eq!(
            serde_json::to_string(&ExportFormat::Json).unwrap(),
            "\"json\""
        );
        assert_eq!(
            serde_json::to_string(&ExportFormat::Csv).unwrap(),
            "\"csv\""
        );
        assert_eq!(
            serde_json::to_string(&ExportFormat::Txt).unwrap(),
            "\"txt\""
        );
    }

    #[test]
    fn test_export_format_deserialisation() {
        assert_eq!(
            serde_json::from_str::<ExportFormat>("\"json\"").unwrap(),
            ExportFormat::Json
        );
        assert_eq!(
            serde_json::from_str::<ExportFormat>("\"csv\"").unwrap(),
            ExportFormat::Csv
        );
        assert_eq!(
            serde_json::from_str::<ExportFormat>("\"txt\"").unwrap(),
            ExportFormat::Txt
        );
    }

    // =========================================================================
    // SearchParams tests
    // =========================================================================

    #[test]
    fn test_search_params_default() {
        let params = SearchParams {
            query: None,
            from_date: None,
            to_date: None,
            enhanced_only: None,
            limit: None,
            offset: None,
        };

        assert!(params.query.is_none());
        assert!(params.limit.is_none());
    }

    #[test]
    fn test_search_params_with_query() {
        let params = SearchParams {
            query: Some("test query".to_string()),
            from_date: None,
            to_date: None,
            enhanced_only: None,
            limit: Some(50),
            offset: Some(10),
        };

        assert_eq!(params.query, Some("test query".to_string()));
        assert_eq!(params.limit, Some(50));
        assert_eq!(params.offset, Some(10));
    }

    #[test]
    fn test_search_params_with_date_range() {
        let params = SearchParams {
            query: None,
            from_date: Some(1705311000), // Unix timestamp
            to_date: Some(1705397400),
            enhanced_only: Some(true),
            limit: None,
            offset: None,
        };

        assert!(params.from_date.is_some());
        assert!(params.to_date.is_some());
        assert_eq!(params.enhanced_only, Some(true));
    }

    #[test]
    fn test_search_params_serialisation() {
        let params = SearchParams {
            query: Some("hello".to_string()),
            from_date: Some(1705311000),
            to_date: None,
            enhanced_only: Some(false),
            limit: Some(100),
            offset: Some(0),
        };

        let json = serde_json::to_string(&params).unwrap();
        let restored: SearchParams = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.query, params.query);
        assert_eq!(restored.from_date, params.from_date);
        assert_eq!(restored.limit, params.limit);
    }

    // =========================================================================
    // SearchResult tests
    // =========================================================================

    #[test]
    fn test_search_result_empty() {
        let result = SearchResult {
            records: Vec::new(),
            total_count: 0,
            has_more: false,
        };

        assert!(result.records.is_empty());
        assert_eq!(result.total_count, 0);
        assert!(!result.has_more);
    }

    #[test]
    fn test_search_result_with_records() {
        let records = vec![
            TranscriptionRecord {
                id: "1".to_string(),
                text: "First".to_string(),
                raw_text: None,
                duration_seconds: None,
                created_at: "2024-01-15T10:30:00".to_string(),
                audio_path: None,
                is_enhanced: false,
                enhancement_prompt: None,
                transcription_model_name: None,
                transcription_duration_seconds: None,
                enhancement_model_name: None,
                enhancement_duration_seconds: None,
            },
            TranscriptionRecord {
                id: "2".to_string(),
                text: "Second".to_string(),
                raw_text: None,
                duration_seconds: None,
                created_at: "2024-01-15T10:31:00".to_string(),
                audio_path: None,
                is_enhanced: false,
                enhancement_prompt: None,
                transcription_model_name: None,
                transcription_duration_seconds: None,
                enhancement_model_name: None,
                enhancement_duration_seconds: None,
            },
        ];

        let result = SearchResult {
            records,
            total_count: 100,
            has_more: true,
        };

        assert_eq!(result.records.len(), 2);
        assert_eq!(result.total_count, 100);
        assert!(result.has_more);
    }

    // =========================================================================
    // Export function tests (with temp files)
    // =========================================================================

    fn create_test_records() -> Vec<TranscriptionRecord> {
        vec![
            TranscriptionRecord {
                id: "id1".to_string(),
                text: "First transcription".to_string(),
                raw_text: Some("first transcription".to_string()),
                duration_seconds: Some(3.5),
                created_at: "2024-01-15T10:30:00".to_string(),
                audio_path: Some("/path/to/audio1.wav".to_string()),
                is_enhanced: true,
                enhancement_prompt: Some("fix-grammar".to_string()),
                transcription_model_name: Some("ggml-large-v3-turbo".to_string()),
                transcription_duration_seconds: Some(1.2),
                enhancement_model_name: Some("llama3.2:3b".to_string()),
                enhancement_duration_seconds: Some(0.8),
            },
            TranscriptionRecord {
                id: "id2".to_string(),
                text: "Second, with \"quotes\"".to_string(),
                raw_text: None,
                duration_seconds: Some(5.0),
                created_at: "2024-01-15T10:35:00".to_string(),
                audio_path: None,
                is_enhanced: false,
                enhancement_prompt: None,
                transcription_model_name: None,
                transcription_duration_seconds: None,
                enhancement_model_name: None,
                enhancement_duration_seconds: None,
            },
        ]
    }

    #[test]
    fn test_export_json_creates_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("export.json");

        let records = create_test_records();
        export_json(&records, &path).expect("Export should succeed");

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("First transcription"));
        assert!(content.contains("id1"));
    }

    #[test]
    fn test_export_json_valid_json() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("export.json");

        let records = create_test_records();
        export_json(&records, &path).expect("Export should succeed");

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Vec<TranscriptionRecord> =
            serde_json::from_str(&content).expect("Should be valid JSON");

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "id1");
    }

    #[test]
    fn test_export_json_empty_records() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("export.json");

        let records: Vec<TranscriptionRecord> = Vec::new();
        export_json(&records, &path).expect("Export should succeed");

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.trim(), "[]");
    }

    #[test]
    fn test_export_csv_creates_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("export.csv");

        let records = create_test_records();
        export_csv(&records, &path).expect("Export should succeed");

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        // Check header
        assert!(content.starts_with("id,text,raw_text,"));
        // Check data
        assert!(content.contains("id1"));
    }

    #[test]
    fn test_export_csv_escapes_special_chars() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("export.csv");

        let records = create_test_records();
        export_csv(&records, &path).expect("Export should succeed");

        let content = std::fs::read_to_string(&path).unwrap();
        // The second record has quotes in text, should be escaped
        assert!(content.contains("\"\"quotes\"\"")); // CSV escapes quotes as ""
    }

    #[test]
    fn test_export_csv_empty_records() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("export.csv");

        let records: Vec<TranscriptionRecord> = Vec::new();
        export_csv(&records, &path).expect("Export should succeed");

        let content = std::fs::read_to_string(&path).unwrap();
        // Should only have header
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("id,text"));
    }

    #[test]
    fn test_export_txt_creates_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("export.txt");

        let records = create_test_records();
        export_txt(&records, &path).expect("Export should succeed");

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("First transcription"));
        assert!(content.contains("Date:"));
    }

    #[test]
    fn test_export_txt_includes_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("export.txt");

        let records = create_test_records();
        export_txt(&records, &path).expect("Export should succeed");

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Duration: 3.5s"));
        assert!(content.contains("Enhanced: Yes"));
    }

    #[test]
    fn test_export_txt_separator_between_records() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("export.txt");

        let records = create_test_records();
        export_txt(&records, &path).expect("Export should succeed");

        let content = std::fs::read_to_string(&path).unwrap();
        // Records should be separated by dashes
        assert!(content.contains("----"));
    }

    #[test]
    fn test_export_txt_empty_records() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("export.txt");

        let records: Vec<TranscriptionRecord> = Vec::new();
        export_txt(&records, &path).expect("Export should succeed");

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn test_export_txt_single_record_no_separator() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("export.txt");

        let records = vec![create_test_records().remove(0)];
        export_txt(&records, &path).expect("Export should succeed");

        let content = std::fs::read_to_string(&path).unwrap();
        // Single record should not have separator
        assert!(!content.contains("----"));
    }
}
