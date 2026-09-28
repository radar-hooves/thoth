//! Insights dashboard aggregation commands.
//!
//! Read-only aggregations over the `transcriptions` table for the Insights
//! pane.  All heavy aggregation is SQL-side; no row text is loaded into Rust.

use crate::TELEMETRY_TARGET;
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::database::{DatabaseError, open_connection};
use crate::error::Error;

// =============================================================================
// Public types
// =============================================================================

/// Time range selector for the insights query.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InsightsRange {
    AllTime,
    Year,
    Month,
    Week,
}

/// Top-level response for `get_insights`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsData {
    pub totals: InsightsTotals,
    pub activity: Vec<DailyActivity>,
    pub current_streak: u32,
    pub longest_streak: u32,
    pub throughput: Vec<ThroughputStats>,
    pub model_usage: ModelUsage,
    pub length_histogram: Vec<HistogramBucket>,
    pub time_of_day: Vec<u32>,
    pub storage: StorageBreakdown,
}

/// Headline summary numbers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsTotals {
    pub total_count: i64,
    /// Sum of `duration_seconds`.
    pub total_audio_seconds: f64,
    /// Approximate word count: `SUM(LENGTH(text)) / 5.5` (SQL-side).
    pub total_words: i64,
    pub enhanced_count: i64,
    /// Estimated typing time saved: `total_words / typing_wpm * 60` seconds.
    pub typing_time_saved_seconds: f64,
    /// Earliest `created_at` in the range, or `None` if no rows exist.
    pub first_recording_at: Option<String>,
}

/// Recordings per day for the activity chart and streak computation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyActivity {
    /// ISO 8601 date string (`YYYY-MM-DD`).
    pub day: String,
    pub count: i64,
    /// Approximate word count for the day.
    pub words: i64,
}

/// Per-backend transcription throughput statistics.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThroughputStats {
    pub name: String,
    pub count: i64,
    pub avg_audio_duration: f64,
    pub avg_processing_time: f64,
    /// Real-time factor: `avg_audio / avg_processing`. Higher = faster.
    pub speed_factor: f64,
}

/// Counts by backend and enhancement prompt.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    /// Recordings per transcription backend.
    pub backend_counts: Vec<BackendCount>,
    /// Usage breakdown by enhancement prompt (enhanced rows only).
    pub enhancement_prompts: Vec<PromptCount>,
    /// Percentage of recordings that were AI-enhanced (0–100).
    pub enhanced_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendCount {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCount {
    pub prompt: String,
    pub count: i64,
}

/// A single bucket in the length histogram.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistogramBucket {
    pub bucket_label: String,
    pub count: i64,
}

/// Disk usage breakdown for the storage card.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageBreakdown {
    pub recordings_bytes: u64,
    pub models_bytes: u64,
    pub db_bytes: u64,
    pub total_bytes: u64,
    /// Oldest audio file mtime in `~/.thoth/Recordings/`, if any.
    pub oldest_recording_at: Option<String>,
}

/// A candidate recording identified as likely cruft by a low text-density check.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CruftCandidate {
    pub id: String,
    pub created_at: String,
    /// First ~40 characters of the transcription text.
    pub text_preview: String,
    pub duration_seconds: f64,
    /// `LENGTH(text) / duration_seconds`; 0.0 for empty text.
    pub density: f64,
    pub audio_path: Option<String>,
    /// File size in bytes; 0 if the file is missing.
    pub file_bytes: u64,
    /// Normalised RMS (0.0–1.0); `None` if the file is missing or unreadable.
    pub rms: Option<f64>,
}

// =============================================================================
// Range helper
// =============================================================================

/// Returns the UTC lower bound for `range`, or `None` for `AllTime`.
///
/// The bound is snapped to the **start of the local calendar day** N days ago
/// rather than a rolling UTC offset.  The activity and streak queries bucket
/// by `date(created_at,'localtime')`, so a rolling UTC cutoff can silently
/// exclude the partial current local day (up to UTC+10 = 10 hours in AEST).
fn range_lower_bound(range: &InsightsRange) -> Option<DateTime<Utc>> {
    let days = match range {
        InsightsRange::AllTime => return None,
        InsightsRange::Year => 365i64,
        InsightsRange::Month => 30,
        InsightsRange::Week => 7,
    };

    // Compute the local calendar date N days ago, then convert its midnight
    // back to UTC so the SQL `created_at >=` comparison is exact.
    let today_local = Local::now().date_naive();
    let start_local = today_local - Duration::days(days);
    // NaiveDate::and_hms_opt(0,0,0) gives local midnight; localtime_to_utc
    // via chrono::Local.
    let start_dt = start_local
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).earliest())
        .map(|local_dt| local_dt.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() - Duration::days(days));

    Some(start_dt)
}

// =============================================================================
// Core aggregation — works against a supplied connection (testable)
// =============================================================================

fn get_insights_with_conn(
    conn: &Connection,
    range: &InsightsRange,
) -> Result<InsightsData, DatabaseError> {
    let lower: Option<String> = range_lower_bound(range).map(|dt| dt.to_rfc3339());

    // --- Totals ---
    let (total_count, total_audio_seconds, total_words_raw, enhanced_count, first_recording_at) =
        if let Some(lb) = &lower {
            conn.query_row(
                r#"
                SELECT
                    COUNT(*),
                    COALESCE(SUM(duration_seconds), 0.0),
                    COALESCE(SUM(LENGTH(text)), 0),
                    COALESCE(SUM(CASE WHEN is_enhanced = 1 THEN 1 ELSE 0 END), 0),
                    MIN(created_at)
                FROM transcriptions
                WHERE created_at >= ?1
                "#,
                params![lb],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )?
        } else {
            conn.query_row(
                r#"
                SELECT
                    COUNT(*),
                    COALESCE(SUM(duration_seconds), 0.0),
                    COALESCE(SUM(LENGTH(text)), 0),
                    COALESCE(SUM(CASE WHEN is_enhanced = 1 THEN 1 ELSE 0 END), 0),
                    MIN(created_at)
                FROM transcriptions
                "#,
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )?
        };

    let total_words = (total_words_raw as f64 / 5.5) as i64;
    let typing_wpm: f64 = 40.0; // no typing_wpm field in config; spec says default 40
    let typing_time_saved_seconds = if typing_wpm > 0.0 {
        total_words as f64 / typing_wpm * 60.0
    } else {
        0.0
    };

    let totals = InsightsTotals {
        total_count,
        total_audio_seconds,
        total_words,
        enhanced_count,
        typing_time_saved_seconds,
        first_recording_at,
    };

    // --- Daily activity ---
    let activity = query_daily_activity(conn, lower.as_deref())?;
    let (current_streak, longest_streak) = compute_streaks(&activity);

    // --- Throughput per backend ---
    let throughput = query_throughput(conn, lower.as_deref())?;

    // --- Model usage ---
    let model_usage = query_model_usage(conn, lower.as_deref(), total_count)?;

    // --- Length histogram ---
    let length_histogram = query_length_histogram(conn, lower.as_deref())?;

    // --- Time of day ---
    let time_of_day = query_time_of_day(conn, lower.as_deref())?;

    // --- Storage ---
    let storage = compute_storage()?;

    Ok(InsightsData {
        totals,
        activity,
        current_streak,
        longest_streak,
        throughput,
        model_usage,
        length_histogram,
        time_of_day,
        storage,
    })
}

fn query_daily_activity(
    conn: &Connection,
    lower: Option<&str>,
) -> Result<Vec<DailyActivity>, DatabaseError> {
    let sql = if lower.is_some() {
        r#"
        SELECT
            date(created_at, 'localtime') AS day,
            COUNT(*) AS cnt,
            COALESCE(CAST(SUM(LENGTH(text)) / 5.5 AS INTEGER), 0) AS words
        FROM transcriptions
        WHERE created_at >= ?1
        GROUP BY day
        ORDER BY day ASC
        "#
        .to_string()
    } else {
        r#"
        SELECT
            date(created_at, 'localtime') AS day,
            COUNT(*) AS cnt,
            COALESCE(CAST(SUM(LENGTH(text)) / 5.5 AS INTEGER), 0) AS words
        FROM transcriptions
        GROUP BY day
        ORDER BY day ASC
        "#
        .to_string()
    };

    let mut stmt = conn.prepare(&sql)?;
    let rows = if let Some(lb) = lower {
        stmt.query_map(params![lb], |row| {
            Ok(DailyActivity {
                day: row.get(0)?,
                count: row.get(1)?,
                words: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], |row| {
            Ok(DailyActivity {
                day: row.get(0)?,
                count: row.get(1)?,
                words: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?
    };
    Ok(rows)
}

/// Compute current and longest streaks from the ordered activity list.
///
/// A "streak" is a maximal contiguous run of calendar days each having >= 1
/// recording. `current_streak` counts backwards from today; if today has no
/// recordings the streak is 0.
fn compute_streaks(activity: &[DailyActivity]) -> (u32, u32) {
    if activity.is_empty() {
        return (0, 0);
    }

    // Collect the set of days that have at least one recording.
    let day_set: HashSet<NaiveDate> = activity
        .iter()
        .filter(|a| a.count > 0)
        .filter_map(|a| NaiveDate::parse_from_str(&a.day, "%Y-%m-%d").ok())
        .collect();

    if day_set.is_empty() {
        return (0, 0);
    }

    // Longest streak: iterate sorted days and count consecutive runs.
    let mut sorted: Vec<NaiveDate> = day_set.iter().cloned().collect();
    sorted.sort_unstable();

    let mut longest = 1u32;
    let mut run = 1u32;
    for window in sorted.windows(2) {
        if window[1] - window[0] == Duration::days(1) {
            run += 1;
            if run > longest {
                longest = run;
            }
        } else {
            run = 1;
        }
    }

    // Current streak: walk backwards from today (local calendar day, matching
    // the localtime-bucketed activity days).
    let today = Local::now().date_naive();
    let mut current = 0u32;
    let mut check = today;
    loop {
        if day_set.contains(&check) {
            current += 1;
            match check.pred_opt() {
                Some(prev) => check = prev,
                None => break,
            }
        } else {
            break;
        }
    }

    (current, longest)
}

fn query_throughput(
    conn: &Connection,
    lower: Option<&str>,
) -> Result<Vec<ThroughputStats>, DatabaseError> {
    let sql = if lower.is_some() {
        r#"
        SELECT
            transcription_model_name,
            COUNT(*) AS cnt,
            COALESCE(AVG(duration_seconds), 0.0) AS avg_audio,
            COALESCE(AVG(transcription_duration_seconds), 0.0) AS avg_proc
        FROM transcriptions
        WHERE transcription_model_name IS NOT NULL
          AND transcription_duration_seconds IS NOT NULL
          AND created_at >= ?1
        GROUP BY transcription_model_name
        ORDER BY cnt DESC
        "#
        .to_string()
    } else {
        r#"
        SELECT
            transcription_model_name,
            COUNT(*) AS cnt,
            COALESCE(AVG(duration_seconds), 0.0) AS avg_audio,
            COALESCE(AVG(transcription_duration_seconds), 0.0) AS avg_proc
        FROM transcriptions
        WHERE transcription_model_name IS NOT NULL
          AND transcription_duration_seconds IS NOT NULL
        GROUP BY transcription_model_name
        ORDER BY cnt DESC
        "#
        .to_string()
    };

    let mut stmt = conn.prepare(&sql)?;
    let rows = if let Some(lb) = lower {
        stmt.query_map(params![lb], map_throughput_row)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], map_throughput_row)?
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(rows)
}

fn map_throughput_row(row: &rusqlite::Row) -> rusqlite::Result<ThroughputStats> {
    let name: String = row.get(0)?;
    let count: i64 = row.get(1)?;
    let avg_audio: f64 = row.get(2)?;
    let avg_proc: f64 = row.get(3)?;
    let speed_factor = if avg_proc > 0.0 {
        avg_audio / avg_proc
    } else {
        0.0
    };
    Ok(ThroughputStats {
        name,
        count,
        avg_audio_duration: avg_audio,
        avg_processing_time: avg_proc,
        speed_factor,
    })
}

fn query_model_usage(
    conn: &Connection,
    lower: Option<&str>,
    total_count: i64,
) -> Result<ModelUsage, DatabaseError> {
    // Per-backend counts
    let backend_counts = {
        let sql = if lower.is_some() {
            r#"
            SELECT
                COALESCE(transcription_model_name, 'unknown') AS name,
                COUNT(*) AS cnt
            FROM transcriptions
            WHERE created_at >= ?1
            GROUP BY transcription_model_name
            ORDER BY cnt DESC
            "#
            .to_string()
        } else {
            r#"
            SELECT
                COALESCE(transcription_model_name, 'unknown') AS name,
                COUNT(*) AS cnt
            FROM transcriptions
            GROUP BY transcription_model_name
            ORDER BY cnt DESC
            "#
            .to_string()
        };

        let mut stmt = conn.prepare(&sql)?;
        if let Some(lb) = lower {
            stmt.query_map(params![lb], |row| {
                Ok(BackendCount {
                    name: row.get(0)?,
                    count: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], |row| {
                Ok(BackendCount {
                    name: row.get(0)?,
                    count: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        }
    };

    // Enhancement prompt breakdown (enhanced rows only)
    let enhancement_prompts = {
        let sql = if lower.is_some() {
            r#"
            SELECT
                COALESCE(enhancement_prompt, 'unknown') AS prompt,
                COUNT(*) AS cnt
            FROM transcriptions
            WHERE is_enhanced = 1
              AND created_at >= ?1
            GROUP BY enhancement_prompt
            ORDER BY cnt DESC
            "#
            .to_string()
        } else {
            r#"
            SELECT
                COALESCE(enhancement_prompt, 'unknown') AS prompt,
                COUNT(*) AS cnt
            FROM transcriptions
            WHERE is_enhanced = 1
            GROUP BY enhancement_prompt
            ORDER BY cnt DESC
            "#
            .to_string()
        };

        let mut stmt = conn.prepare(&sql)?;
        if let Some(lb) = lower {
            stmt.query_map(params![lb], |row| {
                Ok(PromptCount {
                    prompt: row.get(0)?,
                    count: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], |row| {
                Ok(PromptCount {
                    prompt: row.get(0)?,
                    count: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
        }
    };

    let enhanced_pct = if total_count > 0 {
        let enhanced_total: i64 = enhancement_prompts.iter().map(|p| p.count).sum();
        enhanced_total as f64 / total_count as f64 * 100.0
    } else {
        0.0
    };

    Ok(ModelUsage {
        backend_counts,
        enhancement_prompts,
        enhanced_pct,
    })
}

fn query_length_histogram(
    conn: &Connection,
    lower: Option<&str>,
) -> Result<Vec<HistogramBucket>, DatabaseError> {
    // Buckets: 0-5, 5-10, 10-20, 20-40, 40-60, 60-120, 120+
    let bucket_sql = r#"
        SELECT
            CASE
                WHEN duration_seconds < 5   THEN '0-5s'
                WHEN duration_seconds < 10  THEN '5-10s'
                WHEN duration_seconds < 20  THEN '10-20s'
                WHEN duration_seconds < 40  THEN '20-40s'
                WHEN duration_seconds < 60  THEN '40-60s'
                WHEN duration_seconds < 120 THEN '60-120s'
                ELSE '120s+'
            END AS bucket,
            COUNT(*) AS cnt
        FROM transcriptions
        WHERE duration_seconds IS NOT NULL
    "#;

    let where_clause = if lower.is_some() {
        "  AND created_at >= ?1\n GROUP BY bucket ORDER BY MIN(duration_seconds)"
    } else {
        " GROUP BY bucket ORDER BY MIN(duration_seconds)"
    };

    let sql = format!("{}{}", bucket_sql, where_clause);
    let mut stmt = conn.prepare(&sql)?;

    let rows: Vec<(String, i64)> = if let Some(lb) = lower {
        stmt.query_map(params![lb], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?
    };

    // Ensure all buckets appear in the correct order, even if empty.
    let ordered_labels = [
        "0-5s", "5-10s", "10-20s", "20-40s", "40-60s", "60-120s", "120s+",
    ];
    let row_map: std::collections::HashMap<String, i64> = rows.into_iter().collect();
    let buckets = ordered_labels
        .iter()
        .map(|label| HistogramBucket {
            bucket_label: label.to_string(),
            count: *row_map.get(*label).unwrap_or(&0),
        })
        .collect();

    Ok(buckets)
}

fn query_time_of_day(conn: &Connection, lower: Option<&str>) -> Result<Vec<u32>, DatabaseError> {
    let sql = if lower.is_some() {
        r#"
        SELECT CAST(strftime('%H', created_at, 'localtime') AS INTEGER) AS hour, COUNT(*) AS cnt
        FROM transcriptions
        WHERE created_at >= ?1
        GROUP BY hour
        "#
        .to_string()
    } else {
        r#"
        SELECT CAST(strftime('%H', created_at, 'localtime') AS INTEGER) AS hour, COUNT(*) AS cnt
        FROM transcriptions
        GROUP BY hour
        "#
        .to_string()
    };

    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(i64, i64)> = if let Some(lb) = lower {
        stmt.query_map(params![lb], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?
    };

    let mut counts = vec![0u32; 24];
    for (hour, cnt) in rows {
        if (0..24).contains(&hour) {
            counts[hour as usize] = cnt as u32;
        }
    }
    Ok(counts)
}

fn compute_storage() -> Result<StorageBreakdown, DatabaseError> {
    let base = super::get_thoth_directory()?;

    let recordings_dir = base.join("Recordings");
    let models_dir = base.join("models");
    let db_path = base.join("thoth.db");

    let recordings_bytes = dir_size_bytes(&recordings_dir);
    let models_bytes = dir_size_bytes(&models_dir);
    let db_bytes = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    let total_bytes = recordings_bytes + models_bytes + db_bytes;

    // Oldest recording by mtime.
    let oldest_recording_at = oldest_file_mtime(&recordings_dir);

    Ok(StorageBreakdown {
        recordings_bytes,
        models_bytes,
        db_bytes,
        total_bytes,
        oldest_recording_at,
    })
}

// =============================================================================
// Cruft detection
// =============================================================================

const DEFAULT_DENSITY_THRESHOLD: f64 = 1.0;

fn get_cruft_candidates_with_conn(
    conn: &Connection,
    density_threshold: f64,
) -> Result<Vec<CruftCandidate>, DatabaseError> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            id,
            created_at,
            SUBSTR(text, 1, 40) AS text_preview,
            duration_seconds,
            CASE
                WHEN LENGTH(TRIM(text)) = 0 THEN 0.0
                ELSE CAST(LENGTH(text) AS REAL) / duration_seconds
            END AS density,
            audio_path
        FROM transcriptions
        WHERE duration_seconds > 0
          AND (
              LENGTH(TRIM(text)) = 0
              OR CAST(LENGTH(text) AS REAL) / duration_seconds < ?1
          )
        ORDER BY created_at DESC
        "#,
    )?;

    let rows = stmt
        .query_map(params![density_threshold], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut candidates = Vec::with_capacity(rows.len());
    for (id, created_at, text_preview, duration_seconds, density, audio_path) in rows {
        let (file_bytes, rms) = match &audio_path {
            Some(path) => {
                let pb = std::path::PathBuf::from(path);
                let file_bytes = std::fs::metadata(&pb).map(|m| m.len()).unwrap_or(0);
                let rms = compute_wav_rms(&pb);
                (file_bytes, rms)
            }
            None => (0, None),
        };

        candidates.push(CruftCandidate {
            id,
            created_at,
            text_preview,
            duration_seconds,
            density,
            audio_path,
            file_bytes,
            rms,
        });
    }

    Ok(candidates)
}

// =============================================================================
// RMS computation
// =============================================================================

/// Decode a WAV file and compute normalised RMS (0.0–1.0).
///
/// Returns `None` if the file cannot be opened or decoded.
pub fn compute_wav_rms(path: &std::path::Path) -> Option<f64> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();

    match spec.sample_format {
        hound::SampleFormat::Float => {
            let samples: Vec<f32> = reader
                .into_samples::<f32>()
                .filter_map(|s| s.ok())
                .collect();
            if samples.is_empty() {
                return None;
            }
            let rms = crate::audio::metering::calculate_rms(&samples);
            Some(rms as f64)
        }
        hound::SampleFormat::Int => {
            let bit_depth = spec.bits_per_sample;
            let max_val = (1i64 << (bit_depth - 1)) as f64;
            let samples: Vec<f32> = reader
                .into_samples::<i32>()
                .filter_map(|s| s.ok())
                .map(|s| (s as f64 / max_val) as f32)
                .collect();
            if samples.is_empty() {
                return None;
            }
            let rms = crate::audio::metering::calculate_rms(&samples);
            Some(rms as f64)
        }
    }
}

// =============================================================================
// Filesystem helpers (storage)
// =============================================================================

fn dir_size_bytes(path: &std::path::Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    walkdir_size(path)
}

fn walkdir_size(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += walkdir_size(&p);
            } else if let Ok(meta) = p.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

fn oldest_file_mtime(dir: &std::path::Path) -> Option<String> {
    if !dir.exists() {
        return None;
    }
    let oldest = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some(mtime)
        })
        .min();

    oldest.map(|t| DateTime::<Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

// =============================================================================
// Tauri command wrappers
// =============================================================================

/// Returns aggregated insights for the Insights dashboard.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn get_insights(range: InsightsRange) -> Result<InsightsData, Error> {
    let conn = open_connection().map_err(|e| {
        tracing::error!("Failed to open DB for insights: {}", e);
        e
    })?;
    get_insights_with_conn(&conn, &range).map_err(|e| {
        tracing::error!("Failed to compute insights: {}", e);
        format!("Failed to compute insights: {}", e).into()
    })
}

/// Returns recordings flagged as likely cruft by a low text-density heuristic.
///
/// Each returned candidate has its WAV decoded for RMS confirmation.
#[tauri::command]
#[tracing::instrument(target = TELEMETRY_TARGET, skip_all, err)]
pub fn get_cruft_candidates(density_threshold: Option<f64>) -> Result<Vec<CruftCandidate>, Error> {
    let threshold = density_threshold.unwrap_or(DEFAULT_DENSITY_THRESHOLD);
    let conn = open_connection().map_err(|e| {
        tracing::error!("Failed to open DB for cruft scan: {}", e);
        e
    })?;
    get_cruft_candidates_with_conn(&conn, threshold).map_err(|e| {
        tracing::error!("Failed to get cruft candidates: {}", e);
        format!("Failed to get cruft candidates: {}", e).into()
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::migrations::run_migrations;
    use rusqlite::Connection;

    fn make_test_db() -> Connection {
        let mut conn = Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("pragmas");
        run_migrations(&mut conn).expect("migrations");
        conn
    }

    /// Seed a row with specific fields for testing.
    #[allow(clippy::too_many_arguments)]
    fn seed_row(
        conn: &Connection,
        id: &str,
        text: &str,
        duration: Option<f64>,
        created_at: &str,
        is_enhanced: bool,
        enhancement_prompt: Option<&str>,
        transcription_model: Option<&str>,
        transcription_duration: Option<f64>,
    ) {
        conn.execute(
            r#"INSERT INTO transcriptions
               (id, text, created_at, is_enhanced, duration_seconds,
                enhancement_prompt, transcription_model_name, transcription_duration_seconds)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
            params![
                id,
                text,
                created_at,
                is_enhanced as i32,
                duration,
                enhancement_prompt,
                transcription_model,
                transcription_duration,
            ],
        )
        .expect("seed row");
    }

    // -------------------------------------------------------------------------
    // Totals
    // -------------------------------------------------------------------------

    #[test]
    fn totals_count_words_and_enhanced() {
        let conn = make_test_db();
        // 11 chars → floor(11/5.5)=2 words; 22 chars → 4 words
        seed_row(
            &conn,
            "t1",
            "hello world",
            Some(5.0),
            "2024-01-01T00:00:00Z",
            false,
            None,
            None,
            None,
        );
        seed_row(
            &conn,
            "t2",
            "hello world foo!",
            Some(10.0),
            "2024-01-02T00:00:00Z",
            true,
            Some("grammar"),
            None,
            None,
        );

        let data = get_insights_with_conn(&conn, &InsightsRange::AllTime).expect("insights");
        assert_eq!(data.totals.total_count, 2);
        assert_eq!(data.totals.enhanced_count, 1);
        assert!(data.totals.total_audio_seconds > 0.0);
        assert!(data.totals.total_words >= 4, "should have >=4 words");
        assert!(data.totals.typing_time_saved_seconds > 0.0);
    }

    #[test]
    fn totals_empty_db_returns_zeros() {
        let conn = make_test_db();
        let data = get_insights_with_conn(&conn, &InsightsRange::AllTime).expect("insights");
        assert_eq!(data.totals.total_count, 0);
        assert_eq!(data.totals.total_words, 0);
        assert_eq!(data.totals.enhanced_count, 0);
        assert!(data.totals.first_recording_at.is_none());
    }

    // -------------------------------------------------------------------------
    // Activity buckets and streaks
    // -------------------------------------------------------------------------

    #[test]
    fn daily_activity_groups_by_day() {
        let conn = make_test_db();
        // Seeded at midday UTC so the local-time bucketing stays on the same
        // calendar day for any timezone within +-11h (covers AEST and the
        // UTC CI runner), keeping the assertions timezone-stable.
        seed_row(
            &conn,
            "a1",
            "x",
            Some(1.0),
            "2024-03-01T12:00:00Z",
            false,
            None,
            None,
            None,
        );
        seed_row(
            &conn,
            "a2",
            "y",
            Some(1.0),
            "2024-03-01T13:00:00Z",
            false,
            None,
            None,
            None,
        );
        seed_row(
            &conn,
            "a3",
            "z",
            Some(1.0),
            "2024-03-02T12:00:00Z",
            false,
            None,
            None,
            None,
        );

        let data = get_insights_with_conn(&conn, &InsightsRange::AllTime).expect("insights");
        assert_eq!(data.activity.len(), 2, "two distinct days");
        let day1 = data
            .activity
            .iter()
            .find(|d| d.day == "2024-03-01")
            .unwrap();
        assert_eq!(day1.count, 2);
        let day2 = data
            .activity
            .iter()
            .find(|d| d.day == "2024-03-02")
            .unwrap();
        assert_eq!(day2.count, 1);
    }

    #[test]
    fn streaks_consecutive_days() {
        let activity = vec![
            DailyActivity {
                day: "2024-01-01".to_string(),
                count: 1,
                words: 0,
            },
            DailyActivity {
                day: "2024-01-02".to_string(),
                count: 2,
                words: 0,
            },
            DailyActivity {
                day: "2024-01-03".to_string(),
                count: 1,
                words: 0,
            },
            // gap
            DailyActivity {
                day: "2024-01-05".to_string(),
                count: 1,
                words: 0,
            },
        ];
        let (_, longest) = compute_streaks(&activity);
        assert_eq!(longest, 3, "longest run is 3 days (Jan 1-3)");
    }

    #[test]
    fn streaks_single_day() {
        let activity = vec![DailyActivity {
            day: "2024-06-01".to_string(),
            count: 1,
            words: 0,
        }];
        let (_, longest) = compute_streaks(&activity);
        assert_eq!(longest, 1);
    }

    #[test]
    fn streaks_empty_returns_zero() {
        assert_eq!(compute_streaks(&[]), (0, 0));
    }

    // -------------------------------------------------------------------------
    // Length histogram
    // -------------------------------------------------------------------------

    #[test]
    fn length_histogram_correct_buckets() {
        let conn = make_test_db();
        // 2s → 0-5s bucket
        seed_row(
            &conn,
            "h1",
            "a",
            Some(2.0),
            "2024-01-01T00:00:00Z",
            false,
            None,
            None,
            None,
        );
        // 7s → 5-10s bucket
        seed_row(
            &conn,
            "h2",
            "b",
            Some(7.0),
            "2024-01-01T01:00:00Z",
            false,
            None,
            None,
            None,
        );
        // 150s → 120s+ bucket
        seed_row(
            &conn,
            "h3",
            "c",
            Some(150.0),
            "2024-01-01T02:00:00Z",
            false,
            None,
            None,
            None,
        );

        let data = get_insights_with_conn(&conn, &InsightsRange::AllTime).expect("insights");
        let hist: std::collections::HashMap<_, _> = data
            .length_histogram
            .iter()
            .map(|b| (b.bucket_label.as_str(), b.count))
            .collect();

        assert_eq!(hist["0-5s"], 1);
        assert_eq!(hist["5-10s"], 1);
        assert_eq!(hist["120s+"], 1);
        assert_eq!(hist["10-20s"], 0);
        // All seven labels must be present.
        assert_eq!(data.length_histogram.len(), 7);
    }

    // -------------------------------------------------------------------------
    // Time of day
    // -------------------------------------------------------------------------

    #[test]
    fn time_of_day_has_24_elements() {
        let conn = make_test_db();
        seed_row(
            &conn,
            "tod1",
            "x",
            Some(1.0),
            "2024-01-01T09:00:00Z",
            false,
            None,
            None,
            None,
        );
        seed_row(
            &conn,
            "tod2",
            "x",
            Some(1.0),
            "2024-01-01T14:00:00Z",
            false,
            None,
            None,
            None,
        );

        let data = get_insights_with_conn(&conn, &InsightsRange::AllTime).expect("insights");
        assert_eq!(data.time_of_day.len(), 24);
        // Buckets are local-time, so assert the totals are preserved rather than
        // specific hours; this keeps the test timezone-independent.
        assert_eq!(data.time_of_day.iter().sum::<u32>(), 2);
    }

    // -------------------------------------------------------------------------
    // Throughput
    // -------------------------------------------------------------------------

    #[test]
    fn throughput_speed_factor_computed() {
        let conn = make_test_db();
        // 10s audio, 2s processing → speed factor 5.0
        seed_row(
            &conn,
            "sp1",
            "x",
            Some(10.0),
            "2024-01-01T00:00:00Z",
            false,
            None,
            Some("ggml-large-v3-turbo"),
            Some(2.0),
        );
        seed_row(
            &conn,
            "sp2",
            "y",
            Some(20.0),
            "2024-01-02T00:00:00Z",
            false,
            None,
            Some("ggml-large-v3-turbo"),
            Some(4.0),
        );

        let data = get_insights_with_conn(&conn, &InsightsRange::AllTime).expect("insights");
        assert_eq!(data.throughput.len(), 1);
        let t = &data.throughput[0];
        assert_eq!(t.name, "ggml-large-v3-turbo");
        assert_eq!(t.count, 2);
        // avg_audio=15, avg_proc=3 → speed_factor=5
        assert!(
            (t.speed_factor - 5.0).abs() < 0.01,
            "speed_factor should be 5.0"
        );
    }

    // -------------------------------------------------------------------------
    // Density filtering (cruft candidates)
    // -------------------------------------------------------------------------

    #[test]
    fn dense_row_not_flagged_as_cruft() {
        let conn = make_test_db();
        // 200-char text over 10s → density 20 chars/sec → NOT cruft
        let long_text = "a".repeat(200);
        seed_row(
            &conn,
            "c1",
            &long_text,
            Some(10.0),
            "2024-01-01T00:00:00Z",
            false,
            None,
            None,
            None,
        );

        let candidates =
            get_cruft_candidates_with_conn(&conn, DEFAULT_DENSITY_THRESHOLD).expect("cruft");
        assert!(
            candidates.iter().all(|c| c.id != "c1"),
            "dense row must not appear in cruft candidates"
        );
    }

    #[test]
    fn empty_text_row_flagged_as_cruft() {
        let conn = make_test_db();
        // Empty text over 5s → density 0 → IS cruft
        seed_row(
            &conn,
            "c2",
            "",
            Some(5.0),
            "2024-01-01T00:00:00Z",
            false,
            None,
            None,
            None,
        );

        let candidates =
            get_cruft_candidates_with_conn(&conn, DEFAULT_DENSITY_THRESHOLD).expect("cruft");
        assert!(
            candidates.iter().any(|c| c.id == "c2"),
            "empty-text row must be flagged as cruft"
        );
    }

    #[test]
    fn low_density_row_flagged_as_cruft() {
        let conn = make_test_db();
        // 3-char text over 30s → density 0.1 → IS cruft (threshold 1.0)
        seed_row(
            &conn,
            "c3",
            "ok.",
            Some(30.0),
            "2024-01-01T00:00:00Z",
            false,
            None,
            None,
            None,
        );

        let candidates =
            get_cruft_candidates_with_conn(&conn, DEFAULT_DENSITY_THRESHOLD).expect("cruft");
        assert!(
            candidates.iter().any(|c| c.id == "c3"),
            "low-density row must be flagged as cruft"
        );
    }

    #[test]
    fn zero_duration_row_not_flagged_as_cruft() {
        let conn = make_test_db();
        // duration=0 must be excluded from the WHERE clause to avoid div-by-zero
        seed_row(
            &conn,
            "c4",
            "",
            Some(0.0),
            "2024-01-01T00:00:00Z",
            false,
            None,
            None,
            None,
        );

        let candidates =
            get_cruft_candidates_with_conn(&conn, DEFAULT_DENSITY_THRESHOLD).expect("cruft");
        assert!(
            candidates.iter().all(|c| c.id != "c4"),
            "zero-duration row must not appear (excluded by WHERE duration_seconds > 0)"
        );
    }

    // -------------------------------------------------------------------------
    // Range filter
    // -------------------------------------------------------------------------

    #[test]
    fn range_filter_excludes_old_rows() {
        let conn = make_test_db();
        // This row is in 2020 — older than any reasonable "year" window.
        seed_row(
            &conn,
            "r1",
            "old",
            Some(1.0),
            "2020-01-01T00:00:00Z",
            false,
            None,
            None,
            None,
        );

        // AllTime should include it.
        let all = get_insights_with_conn(&conn, &InsightsRange::AllTime).expect("all");
        assert_eq!(all.totals.total_count, 1);

        // Week/Month/Year windows should exclude a 2020 row entirely.
        let week = get_insights_with_conn(&conn, &InsightsRange::Week).expect("week");
        assert_eq!(
            week.totals.total_count, 0,
            "2020 row must be outside Week window"
        );

        let month = get_insights_with_conn(&conn, &InsightsRange::Month).expect("month");
        assert_eq!(
            month.totals.total_count, 0,
            "2020 row must be outside Month window"
        );

        let year = get_insights_with_conn(&conn, &InsightsRange::Year).expect("year");
        assert_eq!(
            year.totals.total_count, 0,
            "2020 row must be outside Year window"
        );
    }

    // -------------------------------------------------------------------------
    // Model usage
    // -------------------------------------------------------------------------

    #[test]
    fn enhanced_pct_calculated_correctly() {
        let conn = make_test_db();
        seed_row(
            &conn,
            "m1",
            "x",
            Some(1.0),
            "2024-01-01T00:00:00Z",
            false,
            None,
            None,
            None,
        );
        seed_row(
            &conn,
            "m2",
            "x",
            Some(1.0),
            "2024-01-02T00:00:00Z",
            true,
            Some("grammar"),
            None,
            None,
        );
        seed_row(
            &conn,
            "m3",
            "x",
            Some(1.0),
            "2024-01-03T00:00:00Z",
            true,
            Some("grammar"),
            None,
            None,
        );
        seed_row(
            &conn,
            "m4",
            "x",
            Some(1.0),
            "2024-01-04T00:00:00Z",
            false,
            None,
            None,
            None,
        );

        let data = get_insights_with_conn(&conn, &InsightsRange::AllTime).expect("insights");
        // 2/4 enhanced → 50%
        assert!(
            (data.model_usage.enhanced_pct - 50.0).abs() < 0.1,
            "expected 50% enhanced, got {}",
            data.model_usage.enhanced_pct
        );
    }

    // -------------------------------------------------------------------------
    // RMS computation
    // -------------------------------------------------------------------------

    #[test]
    fn rms_missing_file_returns_none() {
        let result = compute_wav_rms(std::path::Path::new("/nonexistent/path/audio.wav"));
        assert!(result.is_none(), "missing file should yield None");
    }

    #[test]
    fn rms_real_wav_returns_some() {
        use hound::{SampleFormat, WavSpec, WavWriter};
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.wav");

        let spec = WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(&path, spec).expect("create wav");
        for _ in 0..1600 {
            writer.write_sample(8000i16).expect("write sample");
        }
        writer.finalize().expect("finalize wav");

        let rms = compute_wav_rms(&path);
        assert!(rms.is_some(), "valid WAV should return Some(rms)");
        assert!(rms.unwrap() > 0.0, "non-silent WAV should have rms > 0");
    }
}
