//! Canonical artifact emission (ADR-0010, TECH-SPEC §4.5).
//!
//! For every scenario the emitter produces canonical `.hurl` text — **that
//! exact text** is what `parse_hurl_file` + `run_entries` execute, so drift
//! between artifact and execution is structurally impossible. Alongside it:
//! the sidecar map (`<slug>.map.json`, schema v1: entry ↔ feature anchor,
//! optional flags, capture names, batch boundaries) and, when the scenario
//! references globals or secrets, a `<slug>.vars` file so the backend team can
//! replay with `hurl --variables-file`. Secrets appear as *names only* —
//! values never enter any artifact (ADR-0005).
//!
//! The canonical format is a compatibility surface, locked by the insta
//! snapshot corpus — emitter changes require deliberate `cargo insta review`.

use std::fmt::Write as _;

use serde::Serialize;

use crate::lower::LoweredScenario;
use crate::step::{StepPayload, StepRef};
use crate::world::World;

/// Sidecar map schema version.
pub const MAP_SCHEMA_VERSION: u32 = 1;

/// One scenario's emitted artifact set.
#[derive(Debug, Clone)]
pub struct Artifact {
    /// File-safe artifact name: `<feature-stem>--<scenario-slug>`.
    pub slug: String,
    /// The canonical `.hurl` text — the executed input (ADR-0010).
    pub hurl_text: String,
    /// The sidecar map (`<slug>.map.json`).
    pub map: SidecarMap,
    /// `<slug>.vars` content, when globals or secrets are referenced.
    pub vars: Option<String>,
}

/// Sidecar map: entry ↔ feature anchors (TECH-SPEC §4.5, schema v1).
#[derive(Debug, Clone, Serialize)]
pub struct SidecarMap {
    /// Schema version ([`MAP_SCHEMA_VERSION`]).
    pub schema: u32,
    /// One record per emitted entry block, in file order.
    pub entries: Vec<MapEntry>,
}

/// One entry block in the artifact.
#[derive(Debug, Clone, Serialize)]
pub struct MapEntry {
    /// 1-based inclusive line range of the entry's hurl text (comments excluded).
    pub hurl_lines: [usize; 2],
    /// The authored feature step this entry came from.
    pub feature: FeatureAnchor,
    /// Whether the step was `optional:`.
    pub optional: bool,
    /// Capture names this entry produces (never values).
    pub captures: Vec<String>,
    /// Batch index within the scenario (segmentation boundaries, ADR-0010).
    pub batch: usize,
    /// 0-based step ordinal *within the batch* — the sidecar↔step link is
    /// explicit, never positional (steps without hurl entries would otherwise
    /// shift the correspondence).
    pub step: usize,
}

/// Feature anchor for one entry.
#[derive(Debug, Clone, Serialize)]
pub struct FeatureAnchor {
    /// Feature file path as authored.
    pub file: String,
    /// 1-based step line.
    pub line: usize,
    /// Step text (keyword stripped).
    pub text: String,
}

/// Emit one scenario's artifact set. `None` when the scenario lowers to no
/// hurl entries (nothing to hand to the engine or the backend team).
pub fn emit(scenario: &LoweredScenario, feature_stem: &str, world: &World) -> Option<Artifact> {
    let slug = format!("{}--{}", slugify(feature_stem), slugify(&scenario.name));
    let has_vars = !scenario.globals.is_empty() || !scenario.secrets.is_empty();

    let mut steps: Vec<(usize, usize, &crate::step::LoweredStep)> = Vec::new();
    for (batch_index, batch) in scenario.batches.iter().enumerate() {
        for (step_index, step) in batch.steps.iter().enumerate() {
            if matches!(
                step.payload,
                StepPayload::HurlEntries(_) | StepPayload::MergedAsserts { .. }
            ) {
                steps.push((batch_index, step_index, step));
            }
        }
    }
    // A mixed scenario may open with another engine's batch — the sidecar's
    // real batch/step indices carry the mapping; no positional assumption holds.
    let (_, _, first_step) = *steps
        .iter()
        .find(|(_, _, s)| matches!(s.payload, StepPayload::HurlEntries(_)))?;

    let mut text = String::new();
    let mut line = 0usize;
    let push_line = |text: &mut String, line: &mut usize, content: &str| {
        text.push_str(content);
        text.push('\n');
        *line += 1;
    };

    push_line(
        &mut text,
        &mut line,
        &format!("# proef artifact — {}", scenario.name),
    );
    push_line(
        &mut text,
        &mut line,
        &format!("# source: {}:{}", first_step.step.file, scenario.line),
    );
    let mut replay = format!("# replay: hurl --test {slug}.hurl");
    if has_vars {
        let _ = write!(replay, " --variables-file {slug}.vars");
    }
    for secret in &scenario.secrets {
        // Placeholders, never values (ADR-0005) — the human fills them in.
        let _ = write!(replay, " --secret {secret}=<value>");
    }
    push_line(&mut text, &mut line, &replay);

    let mut entries = Vec::new();
    let mut index = 0usize;
    while index < steps.len() {
        let (batch_index, step_index, step) = steps[index];
        let StepPayload::HurlEntries(payload) = &step.payload else {
            // A merged-asserts step before any request cannot lower (the
            // `then_before_when` diagnostic fires) — nothing to render.
            index += 1;
            continue;
        };
        push_line(&mut text, &mut line, "");
        push_line(
            &mut text,
            &mut line,
            &entry_comment(&step.step, step.label.as_deref()),
        );
        if step.optional {
            push_line(&mut text, &mut line, "# optional");
        }
        let body: Vec<&str> = trimmed_lines(payload);
        let start = line + 1;
        for body_line in &body {
            push_line(&mut text, &mut line, body_line);
        }
        entries.push(MapEntry {
            hurl_lines: [start, line],
            feature: FeatureAnchor {
                file: step.step.file.to_string(),
                line: step.step.line,
                text: step.step.text.to_string(),
            },
            optional: step.optional,
            captures: capture_names(&body),
            batch: batch_index,
            step: step_index,
        });

        // Merged-asserts steps own the trailing assert lines of the entry
        // just rendered (§2.7); their text is already inside `body`.
        index += 1;
        let first_merged = index;
        while index < steps.len()
            && matches!(steps[index].2.payload, StepPayload::MergedAsserts { .. })
        {
            index += 1;
        }
        entries.extend(merged_map_entries(&steps[first_merged..index], line));
    }

    Some(Artifact {
        hurl_text: text,
        map: SidecarMap {
            schema: MAP_SCHEMA_VERSION,
            entries,
        },
        vars: has_vars.then(|| vars_content(scenario, &slug, world)),
        slug,
    })
}

/// Sidecar rows for the merged-asserts steps that follow one rendered entry
/// (§2.7): line spans are assigned back-to-front from the entry's last line
/// `entry_end` — the last merge sits closest to the end.
fn merged_map_entries(
    followers: &[(usize, usize, &crate::step::LoweredStep)],
    entry_end: usize,
) -> Vec<MapEntry> {
    let mut end = entry_end;
    let mut spans: Vec<[usize; 2]> = Vec::new();
    for &(_, _, merged) in followers.iter().rev() {
        let StepPayload::MergedAsserts { lines } = merged.payload else {
            continue;
        };
        spans.push([end.saturating_sub(lines) + 1, end]);
        end = end.saturating_sub(lines);
    }
    spans.reverse();
    followers
        .iter()
        .zip(spans)
        .map(|(&(batch, step, merged), span)| MapEntry {
            hurl_lines: span,
            feature: FeatureAnchor {
                file: merged.step.file.to_string(),
                line: merged.step.line,
                text: merged.step.text.to_string(),
            },
            optional: merged.optional,
            captures: Vec::new(),
            batch,
            step,
        })
        .collect()
}

/// `# <file>:<line> — <step text>` (plus the pack entry label when present).
fn entry_comment(step: &StepRef, label: Option<&str>) -> String {
    match label {
        Some(label) => format!("# {}:{} — {} ({label})", step.file, step.line, step.text),
        None => format!("# {}:{} — {}", step.file, step.line, step.text),
    }
}

/// Payload lines with trailing blank lines dropped (internal lines verbatim —
/// they are already-validated hurl).
fn trimmed_lines(payload: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = payload.lines().collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines
}

/// Capture names declared in `[Captures]` sections (a textual scan over our
/// own canonical text — the engine parses it for real).
fn capture_names(body: &[&str]) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_captures = false;
    for line in body {
        let trimmed = line.trim();
        if trimmed == "[Captures]" {
            in_captures = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_captures = false;
            continue;
        }
        // A new entry (method/status line or comment) ends the section — a
        // stray `k: v`-shaped line after it must not read as a capture.
        if starts_entry_line(trimmed) {
            in_captures = false;
            continue;
        }
        if in_captures
            && let Some((name, _)) = trimmed.split_once(':')
            && !name.trim().is_empty()
            && !name.trim().contains(char::is_whitespace)
        {
            names.push(name.trim().to_owned());
        }
    }
    names
}

/// Does this canonical-emission line open a new request or response (ending
/// any `[Captures]` run)?
fn starts_entry_line(trimmed: &str) -> bool {
    const STARTERS: &[&str] = &[
        "GET ", "POST ", "PUT ", "DELETE ", "PATCH ", "HEAD ", "OPTIONS ", "HTTP ", "HTTP/",
    ];
    trimmed.starts_with('#') || STARTERS.iter().any(|s| trimmed.starts_with(s))
}

/// Filenames referenced as hurl `file,<name>;` bodies or multipart parts in
/// the artifact text. Stock `hurl --test <file>` resolves them relative to the
/// `.hurl` file, so callers copy these next to emitted artifacts to keep the
/// hand-off self-contained (ADR-0010).
pub fn file_references(hurl_text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for line in hurl_text.lines() {
        let mut rest = line;
        while let Some(position) = rest.find("file,") {
            let tail = &rest[position + "file,".len()..];
            let Some(end) = tail.find(';') else { break };
            let name = tail[..end].trim();
            if !name.is_empty() && !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
            rest = &tail[end + 1..];
        }
    }
    names
}

/// `<slug>.vars`: referenced globals as `name=value` (value from the World at
/// emit time), secrets as names only (ADR-0005).
fn vars_content(scenario: &LoweredScenario, slug: &str, world: &World) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "# proef variables for {slug}.hurl");
    for name in &scenario.globals {
        match world.get(name) {
            Some(value) => {
                let _ = writeln!(out, "{name}={value}");
            }
            None => {
                let _ = writeln!(out, "# global `{name}` was unset at emit time\n{name}=");
            }
        }
    }
    for name in &scenario.secrets {
        let _ = writeln!(
            out,
            "# secret `{name}` — supply at replay: --secret {name}=<value>"
        );
    }
    out
}

/// File-safe slug: lowercase alphanumerics, everything else collapses to `-`.
pub fn slugify(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    let mut dash_pending = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if dash_pending && !slug.is_empty() {
                slug.push('-');
            }
            dash_pending = false;
            slug.extend(c.to_lowercase());
        } else {
            dash_pending = true;
        }
    }
    slug
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use super::*;
    use crate::engine::EngineId;
    use crate::step::{LoweredStep, StepBatch, StepKindId, StepRef};
    use crate::world::{GlobalStore, Value};

    fn step(
        line: usize,
        text: &str,
        payload: &str,
        optional: bool,
        label: Option<&str>,
    ) -> LoweredStep {
        LoweredStep {
            step: StepRef {
                file: Arc::from("tests/features/demo.feature"),
                line,
                text: Arc::from(text),
            },
            kind: StepKindId::from("hurl"),
            payload: StepPayload::HurlEntries(payload.to_owned()),
            optional,
            when: None,
            label: label.map(ToOwned::to_owned),
            save_as: BTreeMap::new(),
        }
    }

    fn scenario() -> LoweredScenario {
        LoweredScenario {
            name: "Search finds a client".to_owned(),
            tags: vec!["api".to_owned()],
            line: 4,
            batches: vec![
                StepBatch {
                    index: 0,
                    engine: EngineId::from("hurl"),
                    steps: vec![step(
                        5,
                        "the service is healthy",
                        "GET http://x/health\nHTTP 200\n\n",
                        true,
                        None,
                    )],
                },
                StepBatch {
                    index: 1,
                    engine: EngineId::from("hurl"),
                    steps: vec![step(
                        6,
                        "I search for \"Jansen\"",
                        "GET http://x/search?q=Jansen\nHTTP 200\n[Captures]\nclientId: jsonpath \"$[0].id\"",
                        false,
                        Some("run the search"),
                    )],
                },
            ],
            secrets: BTreeSet::from(["apiToken".to_owned()]),
            globals: BTreeSet::from(["envName".to_owned()]),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn capture_scan_ends_at_the_next_entry() {
        let body = [
            "GET http://x/a",
            "HTTP 200",
            "[Captures]",
            "id: jsonpath \"$.id\"",
            "",
            "# — next request",
            "GET http://x/b",
            "HTTP 200",
        ];
        assert_eq!(capture_names(&body), vec!["id"]);
    }

    #[test]
    fn file_references_finds_file_bodies_and_multipart_parts() {
        let text = "POST http://x/upload\n[Multipart]\nphoto: file,fixture.jpg;\nHTTP 201\n\nPOST http://x/raw\nfile,payload.bin;\nHTTP 200\n";
        assert_eq!(
            file_references(text),
            vec!["fixture.jpg".to_owned(), "payload.bin".to_owned()]
        );
    }

    #[test]
    fn canonical_layout_map_and_vars() {
        let mut store = GlobalStore::new();
        store.insert("envName", Value::String("staging".into()));
        let world = World::new(store);

        let artifact = emit(&scenario(), "500_demo", &world).unwrap();
        assert_eq!(artifact.slug, "500-demo--search-finds-a-client");

        let lines: Vec<&str> = artifact.hurl_text.lines().collect();
        assert_eq!(lines[0], "# proef artifact — Search finds a client");
        assert_eq!(lines[1], "# source: tests/features/demo.feature:4");
        assert!(lines[2].contains("--variables-file"), "{}", lines[2]);
        assert_eq!(
            lines[4],
            "# tests/features/demo.feature:5 — the service is healthy"
        );
        assert_eq!(lines[5], "# optional");
        assert_eq!(lines[6], "GET http://x/health");

        // Map: line ranges point at the hurl text (comments excluded), 1-based.
        let map = &artifact.map;
        assert_eq!(map.schema, 1);
        assert_eq!(map.entries.len(), 2);
        assert_eq!(map.entries[0].hurl_lines, [7, 8]);
        assert!(map.entries[0].optional);
        assert_eq!(map.entries[0].batch, 0);
        assert_eq!(map.entries[1].captures, vec!["clientId"]);
        assert_eq!(map.entries[1].batch, 1);
        let [start, end] = map.entries[1].hurl_lines;
        assert_eq!(lines[start - 1], "GET http://x/search?q=Jansen");
        assert_eq!(end - start, 3);

        // Vars: global value baked, secret as name only.
        let vars = artifact.vars.unwrap();
        assert!(vars.contains("envName=staging"), "{vars}");
        assert!(vars.contains("--secret apiToken=<value>"), "{vars}");
        assert!(!vars.contains("apiToken=\n"), "secret values never appear");
    }

    #[test]
    fn no_hurl_entries_means_no_artifact() {
        let empty = LoweredScenario {
            name: "n".to_owned(),
            tags: Vec::new(),
            line: 1,
            batches: Vec::new(),
            secrets: BTreeSet::new(),
            globals: BTreeSet::new(),
            warnings: Vec::new(),
        };
        assert!(emit(&empty, "f", &World::default()).is_none());
    }

    #[test]
    fn slugs_are_file_safe_and_stable() {
        assert_eq!(slugify("500_api message — sync!"), "500-api-message-sync");
        assert_eq!(slugify("Ütf ærgh"), "ütf-ærgh");
        assert_eq!(slugify("  --  "), "");
    }

    #[test]
    fn emission_is_deterministic() {
        let world = World::default();
        let a = emit(&scenario(), "500_demo", &world).unwrap();
        let b = emit(&scenario(), "500_demo", &world).unwrap();
        assert_eq!(a.hurl_text, b.hurl_text);
        assert_eq!(
            serde_json::to_string(&a.map).unwrap(),
            serde_json::to_string(&b.map).unwrap()
        );
    }
}
