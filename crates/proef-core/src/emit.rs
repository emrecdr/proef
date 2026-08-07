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

use crate::lower::{LoweredScenario, is_method_line};
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
/// (§2.7): the merges own the entry's trailing lines in order, so spans are
/// one forward pass from `entry_end - total`. The caller guarantees every
/// follower is `MergedAsserts` (that is how the range was delimited).
fn merged_map_entries(
    followers: &[(usize, usize, &crate::step::LoweredStep)],
    entry_end: usize,
) -> Vec<MapEntry> {
    let total: usize = followers
        .iter()
        .map(|&(_, _, merged)| match merged.payload {
            StepPayload::MergedAsserts { lines } => lines,
            _ => unreachable!("followers are delimited by the MergedAsserts match"),
        })
        .sum();
    let mut start = entry_end.saturating_sub(total) + 1;
    followers
        .iter()
        .map(|&(batch, step, merged)| {
            let StepPayload::MergedAsserts { lines } = merged.payload else {
                unreachable!("followers are delimited by the MergedAsserts match");
            };
            let span = [start, start + lines - 1];
            start += lines;
            MapEntry {
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
            }
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
///
/// Fence-aware: a fenced (```…```) body is opaque to the scan — a literal
/// `[Captures]` line inside a docstring body must not re-arm it — and a
/// custom-method entry line ends the previous entry via [`is_method_line`],
/// the same recogniser the lowering pass uses. Otherwise phantom rows reach
/// `.map.json`, a normative artifact (ADR-0010).
fn capture_names(body: &[&str]) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_captures = false;
    let mut in_fence = false;
    for line in body {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            in_captures = false;
            continue;
        }
        if in_fence {
            continue;
        }
        if trimmed == "[Captures]" {
            in_captures = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_captures = false;
            continue;
        }
        // A capture-shaped line inside an open run is a capture, full stop —
        // checked ahead of `starts_entry_line` because the generic recogniser
        // cannot tell an uppercase capture name (hurl permits a space before
        // its `:`) from a custom-method entry line, and must not be allowed
        // to guess wrong on this scan's own territory.
        if in_captures && let Some(name) = capture_name(trimmed) {
            names.push(name.to_owned());
            continue;
        }
        // A new entry (method/status line or comment) ends the section — a
        // stray `k: v`-shaped line after it must not read as a capture.
        if starts_entry_line(trimmed) {
            in_captures = false;
            continue;
        }
        // So does a body opener: nothing after it in this entry is a capture.
        if trimmed.starts_with('{') || trimmed.starts_with('<') {
            in_captures = false;
        }
    }
    names
}

/// Is `trimmed` shaped like a capture definition (`name: query`)? Bare
/// identifiers only — a JSON body line (`"status": "ok"`) must never read as
/// the capture `"status"`, and an entry-opening line never has this shape (a
/// method line's first token carries no colon; a response line has no colon
/// at all).
fn capture_name(trimmed: &str) -> Option<&str> {
    let (name, _) = trimmed.split_once(':')?;
    let name = name.trim();
    (!name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    .then_some(name)
}

/// Does this canonical-emission line open a new request, response, or
/// comment (ending any `[Captures]` run)? Requests are recognised via
/// [`is_method_line`] — the lowering pass's recogniser — so a custom method
/// (`PROPFIND`, …) ends the scan exactly as it ends an entry there. The
/// response check requires its own delimiter (`HTTP ` / `HTTP/`) rather than
/// a bare prefix match — a capture merely *named* starting with `HTTP`
/// (`HTTPStatus: …`) is not a response line, and must not be read as one.
fn starts_entry_line(trimmed: &str) -> bool {
    trimmed.starts_with('#')
        || trimmed.starts_with("HTTP ")
        || trimmed.starts_with("HTTP/")
        || is_method_line(trimmed)
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
                let rendered = value.to_string();
                if rendered.contains(['\n', '\r']) {
                    // A raw newline would corrupt the `name=value` line format
                    // `hurl --variables-file` parses — degrade like an unset
                    // global, with the reason on record.
                    let _ = writeln!(
                        out,
                        "# global `{name}` is not line-representable (value contains a newline)\n{name}="
                    );
                } else {
                    let _ = writeln!(out, "{name}={rendered}");
                }
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
            name: "Search finds a record".to_owned(),
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
                        "GET http://x/search?q=Jansen\nHTTP 200\n[Captures]\nrecordId: jsonpath \"$[0].id\"",
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
    fn capture_scan_ignores_fenced_lines_and_ends_at_custom_methods() {
        // A fenced `[Captures]` must not re-arm the scan, and a custom method
        // must end the previous entry — otherwise phantom rows reach the
        // sidecar, which is a normative artifact (ADR-0010).
        let body = [
            "GET http://x/a",
            "HTTP 200",
            "[Captures]",
            "real: jsonpath \"$.id\"",
            "",
            "PROPFIND http://x/b",
            "```",
            "[Captures]",
            "phantom: jsonpath \"$.nope\"",
            "```",
            "HTTP 207",
        ];
        let names = capture_names(&body);
        assert!(names.contains(&"real".to_owned()), "{names:?}");
        assert!(
            !names.contains(&"phantom".to_owned()),
            "fenced capture leaked into the sidecar: {names:?}"
        );
    }

    #[test]
    fn capture_names_keeps_a_capture_whose_name_starts_with_http() {
        // End-to-end invariant: a capture merely *named* starting with
        // "HTTP" (e.g. `HTTPStatus`) must survive the scan, or it — and
        // every capture after it in the entry — is silently missing from
        // the sidecar (ADR-0010: no legitimate row may be dropped). This
        // goes green via the capture-shape guard in `capture_names`, which
        // recognises `HTTPStatus: …` as a capture before `starts_entry_line`
        // is ever consulted; `starts_entry_line_requires_a_delimiter_after_http`
        // pins the response-line predicate itself.
        let body = [
            "GET http://x/a",
            "HTTP 200",
            "[Captures]",
            "HTTPStatus: jsonpath \"$.status\"",
            "plain: jsonpath \"$.id\"",
        ];
        assert_eq!(
            capture_names(&body),
            vec!["HTTPStatus".to_owned(), "plain".to_owned()]
        );
    }

    #[test]
    fn starts_entry_line_requires_a_delimiter_after_http() {
        // Pins the predicate directly: the response check must require its
        // own delimiter (`HTTP ` / `HTTP/`), not a bare prefix match, or it
        // misreads a capture merely *named* starting with `HTTP` as a
        // response line. Real response lines must still match.
        assert!(!starts_entry_line("HTTPStatus: jsonpath \"$.status\""));
        assert!(starts_entry_line("HTTP 200"));
        assert!(starts_entry_line("HTTP/1.1 200"));
        // And a custom-method entry line must be recognised too — this is
        // what `is_method_line` (shared with the lowering pass) buys over a
        // fixed prefix list of the stock HTTP verbs.
        assert!(starts_entry_line("PROPFIND http://x/b"));
    }

    #[test]
    fn capture_scan_ends_the_previous_entry_at_a_custom_method_line() {
        // Direct (unfenced) reproduction of the phantom-row hazard: an open
        // `[Captures]` run must not survive past a custom-method entry line.
        // A blank line does not close the run on its own (only an
        // entry-opening line, a body opener, or a new bracketed section
        // does), so if `PROPFIND` were not recognised as one, `in_captures`
        // would still be armed when the scan reaches `Depth: 1` — a plain
        // request header of the *new* entry — and misread it as a capture
        // named `Depth`. That phantom row would then reach `.map.json`, a
        // normative artifact (ADR-0010). No fence is involved, so this is
        // blind to whether fencing alone happens to save the day.
        let body = [
            "GET http://x/a",
            "HTTP 200",
            "[Captures]",
            "real: jsonpath \"$.id\"",
            "PROPFIND http://x/b",
            "Depth: 1",
            "HTTP 207",
        ];
        let names = capture_names(&body);
        assert_eq!(
            names,
            vec!["real".to_owned()],
            "a custom-method entry line must end the previous entry's capture scan: {names:?}"
        );
    }

    #[test]
    fn capture_names_with_a_space_before_the_colon_are_not_mistaken_for_a_method_line() {
        // hurl's own grammar permits whitespace between a capture's name and
        // its `:` (space0/space1 in hurl_core's `capture()` parser), so an
        // all-uppercase capture name written that way (`STATUS : …`) has
        // exactly the shape `is_method_line` looks for (a ≥3-char uppercase
        // word followed by another token) — it must still read as a capture,
        // not as a new entry that ends the scan (ADR-0010: no legitimate row
        // may be dropped from the sidecar).
        let body = [
            "GET http://x/a",
            "HTTP 200",
            "[Captures]",
            "STATUS : jsonpath \"$.s\"",
            "plain: jsonpath \"$.id\"",
        ];
        assert_eq!(
            capture_names(&body),
            vec!["STATUS".to_owned(), "plain".to_owned()]
        );
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
        assert_eq!(artifact.slug, "500-demo--search-finds-a-record");

        let lines: Vec<&str> = artifact.hurl_text.lines().collect();
        assert_eq!(lines[0], "# proef artifact — Search finds a record");
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
        assert_eq!(map.entries[1].captures, vec!["recordId"]);
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
