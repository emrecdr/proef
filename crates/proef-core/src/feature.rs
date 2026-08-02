//! Feature front end (TECH-SPEC §4.2, §4.4, §7): gherkin parse, tag
//! accumulation, Background prepending, Rule pass-through, Scenario Outline
//! expansion, and data-table capture.
//!
//! Span discipline (TECH-SPEC §9): the gherkin crate's `Span` is 0-based byte
//! offsets (end-exclusive) into the **normalized** source (a trailing newline
//! is appended when missing) — this module normalizes identically, attaches the
//! normalized text to every diagnostic, and clamps. `LineCol` is char-counted
//! and is never used in byte math; parse-error positions are converted by
//! walking the line's `char_indices`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use gherkin::GherkinEnv;

use crate::diag::{Diag, Span};

/// A parsed, fully-expanded feature file: outlines are concrete scenarios,
/// Background steps are prepended, Rule scenarios are inlined.
#[derive(Debug, Clone)]
pub struct FeatureFile {
    /// Feature name as authored.
    pub name: String,
    /// Path as authored (diagnostics + step anchors).
    pub path: String,
    /// Normalized source text (trailing newline guaranteed).
    pub source: Arc<str>,
    /// Feature-level tags (without `@`).
    pub tags: Vec<String>,
    /// All concrete scenarios, in authored order.
    pub scenarios: Vec<ScenarioDef>,
}

/// One concrete (post-expansion) scenario.
#[derive(Debug, Clone)]
pub struct ScenarioDef {
    /// Scenario name (outline placeholders substituted; `#N` suffix added only
    /// when expansion would produce duplicate names).
    pub name: String,
    /// Accumulated tags: feature + rule + scenario + examples (without `@`).
    pub tags: Vec<String>,
    /// Steps, Background-first, in authored order.
    pub steps: Vec<StepDefn>,
    /// 1-based line of the scenario header (display).
    pub line: usize,
}

/// One authored step, ready for binding.
#[derive(Debug, Clone)]
pub struct StepDefn {
    /// Step text (keyword stripped, outline placeholders substituted).
    pub text: String,
    /// Data-table rows (outline placeholders substituted), when present.
    pub table: Option<Vec<Vec<String>>>,
    /// Docstring, when present (raw request bodies).
    pub docstring: Option<String>,
    /// 1-based line of the step (anchors + events).
    pub line: usize,
    /// Byte span in the normalized source.
    pub span: Span,
}

/// Parse one feature file into concrete scenarios. All diagnostics carry the
/// normalized source and byte spans.
pub fn parse(path: &str, text: &str) -> Result<FeatureFile, Vec<Diag>> {
    // A UTF-8 BOM would shift every byte span (and confuse the gherkin
    // parser's first line) — normalization strips it.
    let mut normalized = text.strip_prefix('\u{feff}').unwrap_or(text).to_owned();
    if !normalized.ends_with('\n') {
        normalized.push('\n');
    }
    let source: Arc<str> = Arc::from(normalized.as_str());
    if normalized.trim().is_empty() {
        return Err(vec![
            Diag::error(
                "proef::feature::empty_file",
                "the feature file is empty — a `Feature:` header and at least one scenario are required",
            )
            .with_source(path.to_owned(), Arc::clone(&source)),
        ]);
    }

    let feature = match gherkin::Feature::parse(&*source, GherkinEnv::default()) {
        Ok(feature) => feature,
        Err(err) => {
            let mut diag = Diag::error(
                "proef::feature::parse",
                format!("the feature file does not parse: {err}"),
            )
            .with_source(path.to_owned(), Arc::clone(&source));
            if let Some(span) = parse_error_span(&err.to_string(), &source) {
                diag = diag.with_span(span);
            }
            return Err(vec![diag]);
        }
    };

    let mut diags: Vec<Diag> = Vec::new();
    let mut scenarios: Vec<ScenarioDef> = Vec::new();

    let feature_background = feature.background.as_ref();
    for scenario in &feature.scenarios {
        expand_scenario(
            scenario,
            &feature.tags,
            &[feature_background],
            path,
            &source,
            &mut scenarios,
            &mut diags,
        );
    }
    for rule in &feature.rules {
        let mut rule_tags = feature.tags.clone();
        rule_tags.extend(rule.tags.iter().cloned());
        for scenario in &rule.scenarios {
            expand_scenario(
                scenario,
                &rule_tags,
                &[feature_background, rule.background.as_ref()],
                path,
                &source,
                &mut scenarios,
                &mut diags,
            );
        }
    }

    if diags
        .iter()
        .any(|d| d.severity == crate::diag::Severity::Error)
    {
        return Err(diags);
    }
    dedup_names(&mut scenarios);
    Ok(FeatureFile {
        name: feature.name.clone(),
        path: path.to_owned(),
        source,
        tags: strip_tag_markers(&feature.tags),
        scenarios,
    })
}

/// Expand one (possibly outlined) scenario into concrete [`ScenarioDef`]s.
// One cohesive listing of the expansion rules; splitting hides the order.
#[allow(clippy::too_many_lines)]
fn expand_scenario(
    scenario: &gherkin::Scenario,
    inherited_tags: &[String],
    backgrounds: &[Option<&gherkin::Background>],
    path: &str,
    source: &Arc<str>,
    out: &mut Vec<ScenarioDef>,
    diags: &mut Vec<Diag>,
) {
    let mut tags = inherited_tags.to_vec();
    tags.extend(scenario.tags.iter().cloned());
    let base_steps: Vec<&gherkin::Step> = backgrounds
        .iter()
        .flatten()
        .flat_map(|b| b.steps.iter())
        .chain(scenario.steps.iter())
        .collect();

    // A scenario is an outline when it carries `Examples` — the gherkin crate
    // attaches those only to outlines, in any language, so it is the reliable,
    // dialect-independent signal and is what makes a localized outline expand.
    // The keyword check is a fallback so an English `Scenario Outline`/`Template`
    // whose `Examples` block is omitted still gets the crisp `no_examples` error
    // instead of being mistaken for a plain scenario. A *localized* outline
    // missing its `Examples` cannot be distinguished from a plain scenario here
    // (gherkin 0.16 keeps its dialect keywords private), so it degrades to an
    // unbound-step error on the leftover `<placeholder>` steps — a worse message,
    // never a silent pass.
    let is_outline = !scenario.examples.is_empty()
        || scenario.keyword.contains("Outline")
        || scenario.keyword.contains("Template");
    if !is_outline && scenario.examples.is_empty() {
        out.push(concrete_scenario(
            scenario,
            &tags,
            &base_steps,
            None,
            path,
            source,
            diags,
        ));
        return;
    }

    if scenario.examples.is_empty() || scenario.examples.iter().all(|e| e.table.is_none()) {
        diags.push(
            Diag::error(
                "proef::feature::no_examples",
                format!("scenario outline `{}` has no Examples rows", scenario.name),
            )
            .with_source(path.to_owned(), Arc::clone(source))
            .with_span(clamp(scenario.span, source)),
        );
        return;
    }

    let mut expanded: Vec<ScenarioDef> = Vec::new();
    for examples in &scenario.examples {
        let Some(table) = &examples.table else {
            continue;
        };
        let Some((header, rows)) = table.rows.split_first() else {
            continue;
        };
        if rows.is_empty() {
            diags.push(
                Diag::error(
                    "proef::feature::no_examples",
                    format!(
                        "scenario outline `{}` has an Examples table with a header but no rows",
                        scenario.name
                    ),
                )
                .with_source(path.to_owned(), Arc::clone(source))
                .with_span(clamp(examples.span, source)),
            );
            continue;
        }
        // Duplicate or empty header names would silently drop columns (the
        // substitution map keeps only the last) — reject them loudly.
        let mut seen = std::collections::BTreeSet::new();
        let mut header_broken = false;
        for name in header {
            let name = name.trim();
            if name.is_empty() || !seen.insert(name) {
                let what = if name.is_empty() {
                    "an empty column name".to_owned()
                } else {
                    format!("duplicate column `{name}`")
                };
                diags.push(
                    Diag::error(
                        "proef::feature::bad_examples_header",
                        format!(
                            "scenario outline `{}`: the Examples header has {what} — every column needs a unique, non-empty name",
                            scenario.name
                        ),
                    )
                    .with_source(path.to_owned(), Arc::clone(source))
                    .with_span(clamp(examples.span, source)),
                );
                header_broken = true;
            }
        }
        if header_broken {
            continue;
        }
        let mut example_tags = tags.clone();
        example_tags.extend(examples.tags.iter().cloned());
        for (row_index, row) in rows.iter().enumerate() {
            if row.len() != header.len() {
                diags.push(
                    Diag::error(
                        "proef::feature::ragged_examples",
                        format!(
                            "scenario outline `{}`: Examples row {} has {} cells, the header has {}",
                            scenario.name,
                            row_index + 1,
                            row.len(),
                            header.len()
                        ),
                    )
                    .with_source(path.to_owned(), Arc::clone(source))
                    .with_span(clamp(examples.span, source)),
                );
                continue;
            }
            let substitutions: BTreeMap<&str, &str> = header
                .iter()
                .map(String::as_str)
                .zip(row.iter().map(String::as_str))
                .collect();
            expanded.push(concrete_scenario(
                scenario,
                &example_tags,
                &base_steps,
                Some(&substitutions),
                path,
                source,
                diags,
            ));
        }
    }

    out.extend(expanded);
}

/// Disambiguate duplicate scenario names feature-wide with `#N` suffixes —
/// names key artifact slugs, console buffers, and events, so two scenarios
/// sharing a name would silently overwrite each other's artifact and drain
/// each other's console output.
fn dedup_names(scenarios: &mut [ScenarioDef]) {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for scenario_def in scenarios.iter() {
        *seen.entry(scenario_def.name.clone()).or_default() += 1;
    }
    // Every name in play — authored and assigned — so a rename can never
    // recreate the collision this function exists to prevent (an authored
    // `Name #1` next to a renamed duplicate of `Name`).
    let mut taken: BTreeSet<String> = scenarios.iter().map(|s| s.name.clone()).collect();
    let mut counters: BTreeMap<String, usize> = BTreeMap::new();
    for scenario_def in scenarios.iter_mut() {
        if seen.get(&scenario_def.name).copied().unwrap_or(0) > 1 {
            let n = counters.entry(scenario_def.name.clone()).or_default();
            let renamed = loop {
                *n += 1;
                let candidate = format!("{} #{n}", scenario_def.name);
                if !taken.contains(&candidate) {
                    break candidate;
                }
            };
            taken.insert(renamed.clone());
            scenario_def.name = renamed;
        }
    }
}

/// Build one concrete scenario, substituting outline placeholders when given.
fn concrete_scenario(
    scenario: &gherkin::Scenario,
    tags: &[String],
    steps: &[&gherkin::Step],
    substitutions: Option<&BTreeMap<&str, &str>>,
    path: &str,
    source: &Arc<str>,
    diags: &mut Vec<Diag>,
) -> ScenarioDef {
    let mut check = |text: &str, span: gherkin::Span, what: &str| -> String {
        match substitutions {
            None => text.to_owned(),
            Some(map) => {
                let (result, unknown) = substitute_placeholders(text, map);
                if let Some(name) = unknown {
                    diags.push(
                        Diag::error(
                            "proef::feature::unknown_placeholder",
                            format!(
                                "{what} references `<{name}>`, which is not an Examples column"
                            ),
                        )
                        .with_source(path.to_owned(), Arc::clone(source))
                        .with_span(clamp(span, source)),
                    );
                }
                result
            }
        }
    };

    let name = check(&scenario.name, scenario.span, "the scenario name");
    let steps = steps
        .iter()
        .map(|step| {
            let text = check(&step.value, step.span, "a step");
            let docstring = step
                .docstring
                .as_ref()
                .map(|d| check(d, step.span, "a docstring"));
            let table = step.table.as_ref().map(|t| {
                t.rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|cell| check(cell, t.span, "a table cell"))
                            .collect()
                    })
                    .collect()
            });
            StepDefn {
                text,
                table,
                docstring,
                line: step.position.line,
                span: clamp(step.span, source),
            }
        })
        .collect();

    ScenarioDef {
        name,
        tags: strip_tag_markers(tags),
        steps,
        line: scenario.position.line,
    }
}

/// Substitute `<col>` placeholders; returns the text and the first unknown
/// placeholder name, if any.
fn substitute_placeholders(
    text: &str,
    substitutions: &BTreeMap<&str, &str>,
) -> (String, Option<String>) {
    let mut out = String::with_capacity(text.len());
    let mut unknown = None;
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('>') {
            Some(close) if !after[..close].contains('<') => {
                let name = &after[..close];
                if let Some(value) = substitutions.get(name.trim()) {
                    out.push_str(value);
                } else {
                    if unknown.is_none() {
                        unknown = Some(name.trim().to_owned());
                    }
                    out.push('<');
                    out.push_str(&after[..=close]);
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    (out, unknown)
}

/// Tags without their `@` marker.
fn strip_tag_markers(tags: &[String]) -> Vec<String> {
    tags.iter()
        .map(|t| t.strip_prefix('@').unwrap_or(t).to_owned())
        .collect()
}

/// Clamp a gherkin span into the normalized source (TECH-SPEC §9).
fn clamp(span: gherkin::Span, source: &str) -> Span {
    Span::clamped(span.start, span.end, source.len())
}

/// Best-effort byte span for a gherkin parse error, extracted from its
/// rendered `Error at {line}:{col}` position (the struct fields are private;
/// col is char-counted, so the byte offset walks `char_indices`).
fn parse_error_span(message: &str, source: &str) -> Option<Span> {
    let at = message.strip_prefix("Error at ")?;
    let (line, rest) = at.split_once(':')?;
    let (col, _) = rest.split_once(':')?;
    let (line, col) = (line.parse::<usize>().ok()?, col.parse::<usize>().ok()?);
    let line_start: usize = source
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum();
    let line_text = source[line_start..].lines().next().unwrap_or("");
    let byte_in_line = line_text
        .char_indices()
        .nth(col.saturating_sub(1))
        .map_or(line_text.len(), |(idx, _)| idx);
    Some(Span::clamped(
        line_start + byte_in_line,
        line_start + byte_in_line + 1,
        source.len(),
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const FEATURE: &str = "@e2e @api\nFeature: Search\n\n  Background:\n    Given the api is available\n\n  @search\n  Scenario: Find a record\n    When I search for \"Jansen\"\n    Then the response status is 200\n\n  Scenario Outline: Statuses\n    When I check <path>\n    Then the response status is <status>\n\n    Examples:\n      | path | status |\n      | /a   | 200    |\n      | /b   | 404    |\n";

    #[test]
    fn tags_background_and_outline_expand() {
        let feature = parse("search.feature", FEATURE).unwrap();
        assert_eq!(feature.tags, vec!["e2e", "api"]);
        assert_eq!(feature.scenarios.len(), 3);

        let first = &feature.scenarios[0];
        assert_eq!(first.tags, vec!["e2e", "api", "search"]);
        assert_eq!(first.steps.len(), 3, "background prepended");
        assert_eq!(first.steps[0].text, "the api is available");

        let expanded = &feature.scenarios[1];
        assert_eq!(expanded.steps[1].text, "I check /a");
        assert_eq!(expanded.steps[2].text, "the response status is 200");
        assert_eq!(feature.scenarios[2].steps[1].text, "I check /b");
    }

    // A localized (`# language:`) feature: the gherkin crate strips the dialect
    // keywords, proef consumes the stripped step text, and a localized outline
    // with `Examples` expands like any other. Accented keywords also exercise
    // the non-ASCII byte-offset path (spans stay byte-correct).
    const FEATURE_FR: &str = "# language: fr\nFonctionnalité: Recherche\n\n  Contexte:\n    \
        Soit l'api est disponible\n\n  Scénario: Trouver un enregistrement\n    \
        Quand je cherche \"Jansen\"\n    Alors le statut est 200\n\n  \
        Plan du scénario: Statuts\n    Quand je vérifie <chemin>\n    \
        Alors le statut est <statut>\n\n    Exemples:\n      | chemin | statut |\n      \
        | /a     | 200    |\n      | /b     | 404    |\n";

    #[test]
    fn localized_gherkin_parses_and_outline_expands() {
        let feature = parse("recherche.feature", FEATURE_FR).unwrap();
        // 1 plain scenario + 2 expanded from the localized outline.
        assert_eq!(feature.scenarios.len(), 3);
        // The localized `Contexte`/`Soit` background prepends, keyword-stripped.
        assert_eq!(feature.scenarios[0].steps[0].text, "l'api est disponible");
        // The localized `Plan du scénario` expanded with `<chemin>` substituted
        // and the `Quand` keyword stripped.
        assert_eq!(feature.scenarios[1].steps[1].text, "je vérifie /a");
        assert_eq!(feature.scenarios[2].steps[1].text, "je vérifie /b");
    }

    #[test]
    fn and_but_steps_parse_as_plain_steps() {
        let text = "Feature: F\n  Scenario: S\n    When I do a thing\n    And I do another\n    Then it worked\n    But not too much\n";
        let feature = parse("f.feature", text).unwrap();
        let steps = &feature.scenarios[0].steps;
        assert_eq!(steps.len(), 4, "And/But bind by text like any step");
        assert_eq!(steps[1].text, "I do another");
    }

    #[test]
    fn unknown_placeholder_is_an_error() {
        let text = "Feature: F\n  Scenario Outline: S\n    When I check <wrong>\n\n    Examples:\n      | path |\n      | /a   |\n";
        let errs = parse("f.feature", text).unwrap_err();
        assert_eq!(errs[0].code, "proef::feature::unknown_placeholder");
    }

    #[test]
    fn outline_without_examples_is_an_error() {
        let text = "Feature: F\n  Scenario Outline: S\n    When I check things\n";
        let errs = parse("f.feature", text).unwrap_err();
        assert_eq!(errs[0].code, "proef::feature::no_examples");
    }

    #[test]
    fn duplicate_examples_header_column_is_an_error() {
        // Without the check the substitution map keeps only the last column
        // and the first silently vanishes.
        let text = "Feature: F\n  Scenario Outline: S\n    When I check <path>\n\n    Examples:\n      | path | path |\n      | /a   | /b   |\n";
        let errs = parse("f.feature", text).unwrap_err();
        assert!(
            errs.iter()
                .any(|d| d.code == "proef::feature::bad_examples_header"),
            "{errs:?}"
        );
    }

    #[test]
    fn empty_feature_file_gets_a_named_error() {
        let errs = parse("f.feature", "  \n\n").unwrap_err();
        assert_eq!(errs[0].code, "proef::feature::empty_file");
    }

    #[test]
    fn utf8_bom_is_stripped_before_parsing_and_spans() {
        let text = "\u{feff}Feature: F\n  Scenario: S\n    When I do a thing\n";
        let feature = parse("f.feature", text).unwrap();
        assert_eq!(feature.name, "F");
        assert!(
            !feature.source.starts_with('\u{feff}'),
            "normalized source must not carry the BOM (it would shift spans)"
        );
    }

    #[test]
    fn ragged_examples_row_is_an_error() {
        let text = "Feature: F\n  Scenario Outline: S\n    When I check <path>\n\n    Examples:\n      | path | status |\n      | /a   |\n";
        let errs = parse("f.feature", text).unwrap_err();
        // The gherkin crate itself rejects ragged tables at parse time; our
        // expansion-time check (`ragged_examples`) backstops pad-behavior
        // changes. Either way it must be a parse-time error.
        assert!(
            errs.iter()
                .any(|d| d.code == "proef::feature::ragged_examples"
                    || d.code == "proef::feature::parse")
        );
    }

    #[test]
    fn malformed_gherkin_reports_a_located_parse_error() {
        let errs = parse("f.feature", "Feature broken\nScenario: S\n").unwrap_err();
        assert_eq!(errs[0].code, "proef::feature::parse");
        assert!(errs[0].source_text.is_some());
    }

    #[test]
    fn duplicate_expanded_names_get_disambiguated() {
        let text = "Feature: F\n  Scenario Outline: Same name\n    When I check <path>\n\n    Examples:\n      | path |\n      | /a   |\n      | /b   |\n";
        let feature = parse("f.feature", text).unwrap();
        assert_eq!(feature.scenarios[0].name, "Same name #1");
        assert_eq!(feature.scenarios[1].name, "Same name #2");
    }

    #[test]
    fn rules_pass_through_with_tag_accumulation() {
        let text =
            "@f\nFeature: F\n  @r\n  Rule: R\n    @s\n    Scenario: S\n      When I do a thing\n";
        let feature = parse("f.feature", text).unwrap();
        assert_eq!(feature.scenarios[0].tags, vec!["f", "r", "s"]);
    }
}
