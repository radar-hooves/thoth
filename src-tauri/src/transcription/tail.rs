//! Recover words the engine dropped from the end of a take.
//!
//! FluidAudio sometimes ends a take early. Decoding the last few seconds on
//! their own recovers the words, so the engine decodes that clip and
//! [`splice_tail`] appends whatever the main transcript lacks. Deterministic
//! word matching, no model.

#![cfg_attr(
    not(all(target_os = "macos", feature = "fluidaudio")),
    allow(dead_code)
)]

/// Seconds of audio at the end of a take decoded on its own.
pub(crate) const TAIL_SECS: f32 = 5.0;

/// Takes at most this long (seconds) are skipped: the tail clip would be the
/// whole take and decode identically.
pub(crate) const TAIL_MIN_TOTAL_SECS: f32 = TAIL_SECS + 1.0;

/// Frame length (seconds) for the end-of-take level check.
const LEVEL_FRAME_SECS: f32 = 0.05;

/// Seconds at the end of the take whose level decides whether it ends in speech.
const END_WINDOW_SECS: f32 = 1.0;

/// The end window's median level must reach this fraction of the take's
/// speech level (90th percentile). Measured on 1,491 takes: splices onto a
/// ratio below 0.22 were invented endings; real recoveries sat at 0.31 or more.
const END_MIN_REL: f32 = 0.30;

/// True when the last second of the take carries speech-level audio.
///
/// The decoder invents words ("the first time", "Thank you") on a silent or
/// near-silent ending, so a tail clip is only trusted when the take itself
/// ends in speech. Level alone cannot tell speech from steady noise.
pub(crate) fn ends_in_speech(samples: &[f32], rate: u32) -> bool {
    let frame = ((LEVEL_FRAME_SECS * rate as f32) as usize).max(1);
    let rms: Vec<f32> = samples
        .chunks_exact(frame)
        .map(|f| (f.iter().map(|s| s * s).sum::<f32>() / frame as f32).sqrt())
        .collect();
    let end_frames = (END_WINDOW_SECS / LEVEL_FRAME_SECS) as usize;
    if rms.len() < end_frames * 2 {
        return false;
    }
    let percentile = |v: &[f32], q: f32| {
        let mut v = v.to_vec();
        v.sort_by(|a, b| a.total_cmp(b));
        v[((v.len() as f32 * q) as usize).min(v.len() - 1)]
    };
    let speech = percentile(&rms, 0.90);
    let level = percentile(&rms[rms.len() - end_frames..], 0.50);
    level >= speech * END_MIN_REL
}

/// True when `stem` (at least four letters) is a strict prefix of `word`.
fn is_stem(stem: &str, word: &str) -> bool {
    stem.chars().count() >= 4 && word.len() > stem.len() && word.starts_with(stem)
}

/// A whitespace token with its comparison form and byte range.
struct Word {
    norm: String,
    start: usize,
    end: usize,
}

fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut start = None;
    let mut push = |s: usize, e: usize| {
        let norm: String = text[s..e]
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect();
        if !norm.is_empty() {
            out.push(Word {
                norm,
                start: s,
                end: e,
            });
        }
    };
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                push(s, i);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        push(s, text.len());
    }
    out
}

/// Append to `main` the words at the end of `tail` that `main` lacks.
///
/// `tail` is the transcript of the last seconds of the same take. The words
/// are aligned where `tail` repeats the end of `main` (two or more words, or
/// one long word that occurs once in `tail`); what follows that overlap is
/// appended. When `tail` ends where `main` ends, has no overlap with the end of
/// `main`, or the overlap is ambiguous, `main` is returned unchanged.
pub(crate) fn splice_tail(main: &str, tail: &str) -> String {
    let main = main.trim_end();
    let mw = words(main);
    let tw = words(tail);
    if mw.is_empty() || tw.is_empty() {
        return main.to_string();
    }

    // Longest L such that the last L words of main equal tail[e-L..e].
    let mut best_len = 0;
    let mut ends: Vec<usize> = Vec::new();
    let mut stems: Vec<usize> = Vec::new();
    for e in 1..=tw.len() {
        let max_l = e.min(mw.len());
        let mut l = 0;
        while l < max_l && mw[mw.len() - 1 - l].norm == tw[e - 1 - l].norm {
            l += 1;
        }
        // The engine can end main mid-word ("everyone" for "everyone's"):
        // let main's last word match a longer tail word it prefixes.
        if l == 0 && is_stem(&mw[mw.len() - 1].norm, &tw[e - 1].norm) {
            l = 1;
            let max_l = e.min(mw.len());
            while l < max_l && mw[mw.len() - 1 - l].norm == tw[e - 1 - l].norm {
                l += 1;
            }
            stems.push(e);
        }
        if l == 0 {
            continue;
        }
        if l > best_len {
            best_len = l;
            ends.clear();
        }
        if l == best_len {
            ends.push(e);
        }
    }
    let anchored = best_len >= 2
        || (best_len == 1 && {
            let w = &mw[mw.len() - 1].norm;
            w.chars().count() >= 5
                && tw.iter().filter(|t| t.norm.starts_with(w.as_str())).count() == 1
        });
    if !anchored || ends.contains(&tw.len()) {
        return main.to_string();
    }
    // Several equally good alignments: take the one that appends least.
    let e = *ends.last().unwrap_or(&tw.len());
    // A stem match replaces main's cut-off last word with the tail's whole one.
    let (extra_from, head_to) = if stems.contains(&e) {
        (e - 1, mw[mw.len() - 1].start)
    } else {
        (e, mw[mw.len() - 1].end)
    };
    let extra = &tail[tw[extra_from].start..tw[tw.len() - 1].end];
    let head = main[..head_to].trim_end();
    let head = if stems.contains(&e) {
        head
    } else {
        head.trim_end_matches(['.', '?', '!', ',', ';', ':'])
    };
    format!("{head} {extra}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_dropped_tail() {
        assert_eq!(
            splice_tail(
                "Check with the orchestrator who's",
                "orchestrator who's currently on the job as well."
            ),
            "Check with the orchestrator who's currently on the job as well."
        );
    }

    #[test]
    fn strips_main_terminal_punctuation_at_the_seam() {
        assert_eq!(
            splice_tail(
                "We ship it on Friday.",
                "ship it on Friday afternoon, I think."
            ),
            "We ship it on Friday afternoon, I think."
        );
    }

    #[test]
    fn unchanged_when_tail_ends_where_main_ends() {
        let main = "We ship it on Friday.";
        assert_eq!(splice_tail(main, "ship it on Friday."), main);
        assert_eq!(splice_tail(main, "So we ship it on friday"), main);
    }

    #[test]
    fn unchanged_without_overlap_or_when_main_has_more() {
        let main = "We ship it on Friday.";
        assert_eq!(splice_tail(main, "something else entirely here"), main);
        assert_eq!(splice_tail(main, "Thank you."), main);
        assert_eq!(splice_tail(main, ""), main);
    }

    #[test]
    fn single_short_word_is_not_an_anchor() {
        let main = "We went to the shop";
        assert_eq!(splice_tail(main, "the dog barked loudly"), main);
    }

    #[test]
    fn single_long_word_anchors_when_unique() {
        assert_eq!(
            splice_tail("Ask the orchestrator", "orchestrator is busy now"),
            "Ask the orchestrator is busy now"
        );
    }

    #[test]
    fn never_duplicates_overlap() {
        assert_eq!(splice_tail("a b c d e f", "d e f g h"), "a b c d e f g h");
    }

    #[test]
    fn cut_off_last_word_is_replaced_by_the_whole_one() {
        assert_eq!(
            splice_tail("and on it goes. Everyone", "on it goes. Everyone's got a"),
            "and on it goes. Everyone's got a"
        );
    }

    fn tone(secs: f32, amp: f32) -> Vec<f32> {
        (0..(16_000.0 * secs) as usize)
            .map(|i| amp * ((i as f32) * 0.3).sin())
            .collect()
    }

    #[test]
    fn take_ending_in_speech_passes_the_guard() {
        let mut take = tone(4.0, 0.2);
        take.extend(tone(2.0, 0.002));
        take.extend(tone(4.0, 0.2));
        assert!(ends_in_speech(&take, 16_000));
    }

    #[test]
    fn take_ending_in_silence_fails_the_guard() {
        let mut take = tone(8.0, 0.2);
        take.extend(tone(3.0, 0.002));
        assert!(!ends_in_speech(&take, 16_000));
    }

    #[test]
    fn empty_main_stays_empty() {
        assert_eq!(splice_tail("", "hello there"), "");
    }
}
