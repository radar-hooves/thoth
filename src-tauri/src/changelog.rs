//! The user-facing changelog, read from `CHANGELOG.md` itself.
//!
//! Thoth already writes a good changelog and nothing in the app ever showed it
//! to anyone (#113). Rather than keep a second set of release notes in step
//! with the first, this parses the one that already exists.
//!
//! `CHANGELOG.md` is embedded at BUILD time with [`include_str!`], not read
//! from disk: the shipped app has no repo beside it, and a Tauri resource
//! would be a second place to remember. The cost is that a changelog edit
//! needs a rebuild to appear, which is true of every release anyway.
//!
//! The parser is deliberately small and format-specific rather than a markdown
//! library. It understands exactly what this file is — Keep a Changelog, with
//! `## [version] - date`, `### Section`, and `- **Lead.** body` bullets — and
//! `the_shipped_changelog_parses` fails the build if that stops being true.

use serde::{Deserialize, Serialize};

/// The changelog, as of the build.
const CHANGELOG: &str = include_str!("../../CHANGELOG.md");

/// One release's notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseNotes {
    /// The version, without brackets: `2026.6.7`.
    pub version: String,
    /// Whatever the heading carries after the dash, verbatim. Usually a date;
    /// `[1.0.0]` says "Swift Version (Archived)".
    pub date: Option<String>,
    pub sections: Vec<NotesSection>,
}

/// An `### Added` / `### Fixed` / `### Note` group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotesSection {
    pub heading: String,
    pub items: Vec<NotesItem>,
}

/// One bullet, split at its bold lead so the UI can weight the two parts
/// without a markdown renderer — the app has no `@html` anywhere and this
/// feature is not the reason to introduce one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotesItem {
    /// The `**bold**` opening, without its asterisks. `None` when a bullet has
    /// no bold lead.
    pub lead: Option<String>,
    /// Everything after the lead, or the whole bullet when there is none.
    pub body: String,
}

/// Parse every released version out of the embedded changelog, newest first.
///
/// `## [Unreleased]` is skipped: it describes work that has not shipped, so
/// nobody running a build should be shown it as what changed.
pub fn releases() -> Vec<ReleaseNotes> {
    parse(CHANGELOG)
}

/// The notes for one version, if the changelog has an entry for it.
pub fn release(version: &str) -> Option<ReleaseNotes> {
    releases().into_iter().find(|r| r.version == version)
}

fn parse(source: &str) -> Vec<ReleaseNotes> {
    let mut releases: Vec<ReleaseNotes> = Vec::new();
    let mut current: Option<ReleaseNotes> = None;

    for line in source.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if let Some(release) = current.take() {
                releases.push(release);
            }
            current = parse_version_heading(rest).map(|(version, date)| ReleaseNotes {
                version,
                date,
                sections: Vec::new(),
            });
            continue;
        }

        let Some(release) = current.as_mut() else {
            continue;
        };

        if let Some(heading) = line.strip_prefix("### ") {
            release.sections.push(NotesSection {
                heading: heading.trim().to_string(),
                items: Vec::new(),
            });
            continue;
        }

        if line.trim().is_empty() {
            continue;
        }

        // Content outside any `###` heading still belongs to the release — a
        // loose bullet, or a release whose whole note is a paragraph, as
        // `[1.0.0]` is. Give it an unnamed section rather than dropping it.
        if release.sections.is_empty() {
            release.sections.push(NotesSection {
                heading: String::new(),
                items: Vec::new(),
            });
        }

        let Some(section) = release.sections.last_mut() else {
            continue;
        };

        if let Some(bullet) = line.strip_prefix("- ") {
            section.items.push(parse_item(bullet));
        } else if let Some(item) = section.items.last_mut() {
            // A wrapped bullet, or a second line of prose, continues the one
            // above it.
            item.body.push(' ');
            item.body.push_str(line.trim());
        } else {
            // Prose with no bullet before it starts the section.
            section.items.push(parse_item(line));
        }
    }

    if let Some(release) = current.take() {
        releases.push(release);
    }
    releases
}

/// `[2026.6.7] - 2026-06-25` -> `("2026.6.7", Some("2026-06-25"))`.
///
/// Returns `None` for anything that is not a released version, which is how
/// `[Unreleased]` and the link-reference footer are skipped.
fn parse_version_heading(heading: &str) -> Option<(String, Option<String>)> {
    let heading = heading.trim();
    let inner = heading.strip_prefix('[')?;
    let (version, rest) = inner.split_once(']')?;
    // A version starts with a digit; `Unreleased` does not.
    if !version.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let date = rest
        .trim()
        .strip_prefix('-')
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());
    Some((version.to_string(), date))
}

/// Split a bullet at its `**bold lead**`.
fn parse_item(bullet: &str) -> NotesItem {
    let bullet = bullet.trim();
    if let Some(rest) = bullet.strip_prefix("**")
        && let Some((lead, body)) = rest.split_once("**")
    {
        return NotesItem {
            lead: Some(lead.trim().to_string()),
            body: body.trim().to_string(),
        };
    }
    NotesItem {
        lead: None,
        body: bullet.to_string(),
    }
}

/// The release notes to show on this launch, if any (#113).
///
/// `Some` only when the running version differs from the one already seen AND
/// the changelog has an entry for it. Both halves matter: a build whose
/// version is not in the changelog (a local `cargo run`, a release whose entry
/// was not written) shows nothing rather than an empty modal.
///
/// A fresh install shows nothing: startup marks the running version as seen
/// before the window exists, because `last_run_version` being absent is the
/// only signal that this is a first launch and it is overwritten moments
/// later.
#[tauri::command]
pub fn whats_new(version: String) -> Option<ReleaseNotes> {
    let seen = crate::config::get_config()
        .ok()
        .and_then(|c| c.general.whats_new_seen_version);
    if seen.as_deref() == Some(version.as_str()) {
        return None;
    }
    release(&version)
}

/// Record that this version's notes have been seen, so they are shown once.
#[tauri::command]
pub fn mark_whats_new_seen(version: String) -> Result<(), crate::error::Error> {
    crate::config::record_whats_new_seen(&version)
}

/// Every release in the changelog, newest first — the "what changed" list the
/// About dialog can offer beyond the one release the modal shows.
#[tauri::command]
pub fn changelog_releases() -> Vec<ReleaseNotes> {
    releases()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parser is format-specific, so it has to be held against the real
    /// file rather than a fixture: a changelog written in a shape it does not
    /// understand would otherwise ship an empty modal and nobody would know.
    #[test]
    fn the_shipped_changelog_parses() {
        let releases = releases();
        assert!(
            releases.len() >= 5,
            "expected the real changelog's releases, got {}",
            releases.len()
        );

        assert!(
            !releases
                .iter()
                .any(|r| r.version.eq_ignore_ascii_case("unreleased")),
            "unreleased work must never be shown as what changed"
        );

        for release in &releases {
            assert!(
                release.version.starts_with(|c: char| c.is_ascii_digit()),
                "version {:?} is not a version",
                release.version
            );
            assert!(
                !release.sections.is_empty(),
                "release {} parsed with no sections",
                release.version
            );
            assert!(
                release.sections.iter().any(|s| !s.items.is_empty()),
                "release {} parsed with no items",
                release.version
            );
        }
    }

    /// Newest first, because that is the order the file is written in and the
    /// order a "what changed" list is read in.
    ///
    /// The expected version is the crate's own, not a literal, so a retyped
    /// version here just rots at the next release. This is also the gate that
    /// catches a release cut without renaming the CHANGELOG's `[Unreleased]`
    /// heading to the new version first: `bump-version.sh` deliberately does
    /// not touch CHANGELOG.md (release notes are hand-written prose, not a
    /// mechanical rewrite), so that rename is a manual step `git-release.md`
    /// takes before running the script — done in the same commit as the
    /// version bump, since both need the same date and version.
    #[test]
    fn releases_come_back_newest_first() {
        let releases = releases();
        assert_eq!(releases.first().unwrap().version, env!("CARGO_PKG_VERSION"));
    }

    /// The whole point of embedding the file: asking for the running version
    /// gets that version's notes.
    #[test]
    fn a_known_version_is_found_and_an_unknown_one_is_not() {
        let found = release("2026.6.7").expect("2026.6.7 is in the changelog");
        assert_eq!(found.date.as_deref(), Some("2026-06-25"));
        assert!(found.sections.iter().any(|s| s.heading == "Fixed"));

        assert!(release("1999.1.0").is_none());
        assert!(release("Unreleased").is_none());
    }

    #[test]
    fn a_bold_lead_is_split_from_its_body() {
        let item = parse_item("**Something broke.** And here is why.");
        assert_eq!(item.lead.as_deref(), Some("Something broke."));
        assert_eq!(item.body, "And here is why.");
    }

    /// A bullet with no bold lead is still a bullet, not an empty row.
    #[test]
    fn a_plain_bullet_keeps_all_its_text() {
        let item = parse_item("Just a plain note.");
        assert_eq!(item.lead, None);
        assert_eq!(item.body, "Just a plain note.");
    }

    /// An unterminated `**` must not swallow the rest of the line.
    #[test]
    fn an_unclosed_bold_lead_is_left_alone() {
        let item = parse_item("**Never closed");
        assert_eq!(item.lead, None);
        assert_eq!(item.body, "**Never closed");
    }

    #[test]
    fn a_version_heading_without_a_date_still_parses() {
        assert_eq!(
            parse_version_heading("[2026.1.0]"),
            Some(("2026.1.0".to_string(), None))
        );
        assert_eq!(parse_version_heading("[Unreleased]"), None);
        assert_eq!(parse_version_heading("Changelog"), None);
    }

    /// A bullet wrapped over several lines is one item, not one per line.
    #[test]
    fn a_wrapped_bullet_is_one_item() {
        let source =
            "## [2026.1.0] - 2026-01-01\n\n### Fixed\n\n- **Lead.** First line\n  second line\n";
        let releases = parse(source);
        let items = &releases[0].sections[0].items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].body, "First line second line");
    }

    /// A bullet before any `###` belongs to the release rather than vanishing.
    #[test]
    fn a_bullet_with_no_section_heading_is_kept() {
        let source = "## [2026.1.0] - 2026-01-01\n\n- A loose note\n";
        let releases = parse(source);
        assert_eq!(releases[0].sections.len(), 1);
        assert_eq!(releases[0].sections[0].heading, "");
        assert_eq!(releases[0].sections[0].items[0].body, "A loose note");
    }

    /// The real changelog's `[1.0.0]` entry is a paragraph with no bullets and
    /// no `###` heading at all. It must not parse to an empty release.
    #[test]
    fn a_release_written_as_prose_is_kept() {
        let source = "## [1.0.0] - Swift Version (Archived)\n\nOriginal implementation.\n";
        let releases = parse(source);
        assert_eq!(releases[0].version, "1.0.0");
        assert_eq!(
            releases[0].date.as_deref(),
            Some("Swift Version (Archived)")
        );
        assert_eq!(
            releases[0].sections[0].items[0].body,
            "Original implementation."
        );
    }
}
