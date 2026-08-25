//! Step binding (TECH-SPEC §4.3): match each authored step against the loaded
//! macros' `match:` patterns.
//!
//! Exactly one pattern must match: zero is an unbound step (with a
//! closest-pattern suggestion), two or more is ambiguity (listing candidates).
//! Captured `{name}` values, `| key | value |` data-table rows, and macro
//! defaults fill the macro's params; conflicts and missing required params are
//! bind-time errors anchored to the step's line.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use crate::diag::{Diag, Severity};
use crate::feature::{FeatureFile, ScenarioDef, StepDefn};
use crate::matcher;
use crate::pack::PackSet;

/// One step bound to a macro with fully-assembled args.
#[derive(Debug, Clone)]
pub struct BoundStep {
    /// The authored step (anchor, span, keyword, table).
    pub defn: StepDefn,
    /// The macro this step invokes.
    pub macro_name: String,
    /// Assembled args: captures + data table + defaults.
    pub args: BTreeMap<String, String>,
}

/// One scenario with every step bound.
#[derive(Debug, Clone)]
pub struct BoundScenario {
    /// Scenario name (post-expansion).
    pub name: String,
    /// Accumulated tags (without `@`).
    pub tags: Vec<String>,
    /// 1-based header line.
    pub line: usize,
    /// Bound steps in authored order.
    pub steps: Vec<BoundStep>,
}

/// Bind every scenario, always returning both the bound scenarios (bound steps
/// only) and every diagnostic. This is the collect-all substrate the LSP reads;
/// `bind` is its fail-fast wrapper. One binder, two error policies.
pub fn bind_collect(feature: &FeatureFile, packs: &PackSet) -> (Vec<BoundScenario>, Vec<Diag>) {
    let mut diags: Vec<Diag> = Vec::new();
    let mut scenarios = Vec::new();
    let defs = packs.step_defs();
    for scenario in &feature.scenarios {
        scenarios.push(bind_scenario(scenario, feature, packs, &defs, &mut diags));
    }
    (scenarios, diags)
}

/// Bind every scenario of a feature. Diagnostics accumulate across steps and
/// scenarios so authors see all problems in one run.
pub fn bind(feature: &FeatureFile, packs: &PackSet) -> Result<Vec<BoundScenario>, Vec<Diag>> {
    let (scenarios, diags) = bind_collect(feature, packs);
    if diags.iter().any(|d| d.severity == Severity::Error) {
        Err(diags)
    } else {
        Ok(scenarios)
    }
}

// One cohesive listing of the binding rules; splitting hides the order.
#[allow(clippy::too_many_lines)]
fn bind_scenario(
    scenario: &ScenarioDef,
    feature: &FeatureFile,
    packs: &PackSet,
    defs: &[(&str, &str)],
    diags: &mut Vec<Diag>,
) -> BoundScenario {
    let mut steps = Vec::new();
    for step in &scenario.steps {
        let at = |diag: Diag| {
            diag.with_source(feature.path.clone(), Arc::clone(&feature.source))
                .with_span(step.span)
        };

        let candidates: Vec<(&str, &str, BTreeMap<String, String>)> = defs
            .iter()
            .filter_map(|(pattern, macro_name)| {
                matcher::match_pattern(pattern, &step.text)
                    .map(|args| (*pattern, *macro_name, args))
            })
            .collect();

        match candidates.len() {
            0 => {
                let suggestion = closest_pattern(&step.text, defs)
                    .map(|p| format!(" — did you mean `{p}`?"))
                    .unwrap_or_default();
                diags.push(
                    at(Diag::error(
                        "proef::bind::unbound_step",
                        format!("no macro matches `{}`{suggestion}", step.text),
                    ))
                    .with_help(macro_stub(&step.text)),
                );
                continue;
            }
            1 => {}
            _ => {
                let listing = candidates
                    .iter()
                    .map(|(pattern, macro_name, _)| format!("`{macro_name}` ({pattern})"))
                    .collect::<Vec<_>>()
                    .join(", ");
                diags.push(
                    at(Diag::error(
                        "proef::bind::ambiguous_step",
                        format!(
                            "`{}` matches {} macros: {listing}",
                            step.text,
                            candidates.len()
                        ),
                    ))
                    .with_help(
                        "make one pattern more specific (add a literal word) or retire the \
                         duplicate — every sentence must bind to exactly one macro",
                    ),
                );
                continue;
            }
        }

        let (_, macro_name, mut args) = candidates.into_iter().next().unwrap_or_default();
        let Some(macro_) = packs.macros.get(macro_name) else {
            continue; // unreachable: step_defs derive from the same map
        };

        // Data-table rows merge into args (`| key | value |`).
        if let Some(rows) = &step.table {
            for row in rows {
                let [key, value] = row.as_slice() else {
                    diags.push(at(Diag::error(
                        "proef::bind::bad_table",
                        format!(
                            "data tables merge as `| key | value |` — this row has {} cells",
                            row.len()
                        ),
                    )));
                    continue;
                };
                if args.contains_key(key) {
                    diags.push(
                        at(Diag::error(
                            "proef::bind::table_conflict",
                            format!("`{key}` is set both by a `{{capture}}` and the data table"),
                        ))
                        .with_help(
                            "one source per param: drop the table row, or remove the \
                         `{capture}` from the `match:` sentence",
                        ),
                    );
                    continue;
                }
                if !macro_.params.contains(key) {
                    let suggestion = matcher::suggest_or_enumerate(
                        key,
                        macro_.params.iter().map(String::as_str),
                        None,
                    );
                    diags.push(at(Diag::error(
                        "proef::bind::unknown_table_key",
                        format!(
                            "`{key}` is not a param of macro `{}`{suggestion}",
                            macro_.name
                        ),
                    )));
                    continue;
                }
                args.insert(key.clone(), value.clone());
            }
        }

        // A docstring feeds the macro's `docstring` param (raw request bodies,
        // TECH-SPEC §7); a macro that doesn't declare it gets a warning.
        if let Some(docstring) = &step.docstring {
            if macro_.params.iter().any(|p| p == "docstring") {
                args.insert("docstring".to_owned(), docstring.clone());
            } else {
                diags.push(at(Diag::warning(
                    "proef::bind::docstring_unused",
                    format!(
                        "this step has a docstring but macro `{}` declares no `docstring` param — ignored",
                        macro_.name
                    ),
                )));
            }
        }

        // Defaults fill the gaps; whatever is still missing is required.
        for (param, default) in &macro_.defaults {
            args.entry(param.clone()).or_insert_with(|| default.clone());
        }
        for param in &macro_.params {
            if !args.contains_key(param) {
                diags.push(at(Diag::error(
                    "proef::bind::missing_param",
                    format!(
                        "macro `{}` needs `{param}` — add a `{{{param}}}` capture, a data-table row, or a default",
                        macro_.name
                    ),
                )));
            }
        }

        steps.push(BoundStep {
            defn: step.clone(),
            macro_name: macro_name.to_owned(),
            args,
        });
    }

    BoundScenario {
        name: scenario.name.clone(),
        tags: scenario.tags.clone(),
        line: scenario.line,
        steps,
    }
}

/// The closest `match:` pattern to an unbound step, comparing against each
/// pattern's literal skeleton within the shared suggestion threshold.
///
/// Capture *values* in the step would inflate a whole-text distance (`I serch
/// for Jansen` is far from the skeleton `I search for`), so the distance is
/// also taken over the step's prefix clipped to the skeleton's char length —
/// the minimum of both comparisons decides.
fn closest_pattern<'a>(step_text: &str, defs: &[(&'a str, &str)]) -> Option<&'a str> {
    defs.iter()
        .map(|(pattern, _)| {
            let skeleton = matcher::literal_skeleton(pattern);
            let skeleton = skeleton.trim();
            let clipped: String = step_text.chars().take(skeleton.chars().count()).collect();
            let distance = matcher::levenshtein(step_text, skeleton)
                .min(matcher::levenshtein(&clipped, skeleton));
            (distance, *pattern)
        })
        .filter(|(distance, _)| *distance <= 3)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, pattern)| pattern)
}

/// A paste-ready pack-macro stub for an unbound step. Quoted tokens become
/// `{argN}` captures (the matcher sheds those quotes when binding), so an author
/// can drop the stub into a pack and fill in the request instead of hand-writing
/// the `match:`/`hurl:` scaffold.
fn macro_stub(step_text: &str) -> String {
    let mut pattern = String::new();
    let mut arg = 0u32;
    let mut chars = step_text.chars();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' {
            // Consume through the matching quote — the quoted run is one capture.
            for q in chars.by_ref() {
                if q == c {
                    break;
                }
            }
            arg += 1;
            let _ = write!(pattern, "{{arg{arg}}}");
        } else {
            pattern.push(c);
        }
    }
    // Two readers, two different actions. A scenario author writes prose
    // against a vocabulary somebody else maintains, so their move is to say
    // something the packs already bind. A pack maintainer's move is the stub
    // below. The author's action leads because they cannot perform the
    // maintainer's; the stub stays because the maintainer needs it verbatim.
    //
    // Names no tool: this text reaches an editor's diagnostics pane verbatim
    // through the LSP as well as the terminal, and each front end already has
    // its own way to show the vocabulary (completion there, `macros` there).
    // Core does not know which one is reading.
    format!(
        "match a sentence the suite's packs already bind, or \
         add a macro to a pack:\n\nmacros:\n  \
         newMacro:\n    match: {pattern}\n    steps:\n      - hurl: |\n          \
         GET ${{url:base}}/PATH\n          HTTP 200"
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::engine::StepKindSpec;
    use crate::pack::{self, PackSource};

    const KINDS: &[StepKindSpec] = &[StepKindSpec {
        prefix: "hurl",
        schema: "true",
        validate: None,
        fragments: None,
        options: None,
    }];

    fn packs() -> PackSet {
        let sources = vec![PackSource {
            name: "test.yaml".into(),
            text: Arc::from(
                "macros:\n  search:\n    params: [term, index]\n    defaults: { index: records }\n    match: \"I search for {term}\"\n    steps:\n      - hurl: |\n          GET http://x/${index}?q=${term}\n          HTTP 200\n",
            ),
        }];
        pack::load(&sources, &crate::pack::FragmentCorpus::empty(), KINDS).unwrap()
    }

    fn make_feature(body: &str) -> FeatureFile {
        crate::feature::parse("t.feature", &format!("Feature: F\n  Scenario: S\n{body}")).unwrap()
    }

    #[test]
    fn macro_stub_parametrizes_quoted_tokens() {
        // Quoted runs become sequential {argN} captures (double and single quotes).
        let stub = macro_stub("the operator searches for \"Acme\" in 'people'");
        assert!(
            stub.contains("match: the operator searches for {arg1} in {arg2}"),
            "{stub}"
        );
        // A quote-free step keeps its literal text as the pattern.
        assert!(
            macro_stub("all done").contains("match: all done"),
            "no-quote stub"
        );
    }

    #[test]
    fn captures_tables_and_defaults_assemble_args() {
        let feature = make_feature("    When I search for \"Jansen\"\n");
        let bound = bind(&feature, &packs()).unwrap();
        let step = &bound[0].steps[0];
        assert_eq!(step.macro_name, "search");
        assert_eq!(step.args["term"], "Jansen");
        assert_eq!(step.args["index"], "records", "default filled");
    }

    #[test]
    fn table_overrides_defaults_but_not_captures() {
        let feature = make_feature("    When I search for Jansen\n      | index | people |\n");
        let bound = bind(&feature, &packs()).unwrap();
        assert_eq!(bound[0].steps[0].args["index"], "people");

        let feature = make_feature("    When I search for Jansen\n      | term | other |\n");
        let errs = bind(&feature, &packs()).unwrap_err();
        assert_eq!(errs[0].code, "proef::bind::table_conflict");
    }

    #[test]
    fn unbound_step_suggests_the_closest_pattern() {
        let feature = make_feature("    When I serch for Jansen\n");
        let errs = bind(&feature, &packs()).unwrap_err();
        assert_eq!(errs[0].code, "proef::bind::unbound_step");
        assert!(
            errs[0].message.contains("I search for {term}"),
            "{}",
            errs[0].message
        );
    }

    #[test]
    fn unknown_table_key_and_bad_table_shape_error() {
        let feature = make_feature("    When I search for Jansen\n      | indx | people |\n");
        let errs = bind(&feature, &packs()).unwrap_err();
        assert_eq!(errs[0].code, "proef::bind::unknown_table_key");
        assert!(errs[0].message.contains("did you mean `index`?"));

        let feature = make_feature("    When I search for Jansen\n      | a | b | c |\n");
        let errs = bind(&feature, &packs()).unwrap_err();
        assert_eq!(errs[0].code, "proef::bind::bad_table");
    }

    #[test]
    fn ambiguity_lists_all_candidates() {
        let sources = vec![PackSource {
            name: "test.yaml".into(),
            text: Arc::from(
                "macros:\n  a:\n    params: [x]\n    match: \"do {x} now\"\n    steps:\n      - hurl: |\n          GET http://x\n  b:\n    params: [x]\n    match: \"do {x} now\"\n    steps:\n      - hurl: |\n          GET http://y\n",
            ),
        }];
        let packs = pack::load(&sources, &crate::pack::FragmentCorpus::empty(), KINDS).unwrap();
        let feature = make_feature("    When do it now\n");
        let errs = bind(&feature, &packs).unwrap_err();
        assert_eq!(errs[0].code, "proef::bind::ambiguous_step");
        assert!(errs[0].message.contains("`a`") && errs[0].message.contains("`b`"));
    }

    #[test]
    fn missing_required_param_is_reported() {
        let sources = vec![PackSource {
            name: "test.yaml".into(),
            text: Arc::from(
                "macros:\n  create:\n    params: [firstName, lastName]\n    match: I create a record\n    steps:\n      - hurl: |\n          POST http://x/${firstName}/${lastName}\n",
            ),
        }];
        let packs = pack::load(&sources, &crate::pack::FragmentCorpus::empty(), KINDS).unwrap();
        let feature = make_feature("    When I create a record\n");
        let errs = bind(&feature, &packs).unwrap_err();
        assert_eq!(errs.len(), 2);
        assert!(errs.iter().all(|d| d.code == "proef::bind::missing_param"));
    }

    #[test]
    fn bind_collect_returns_bindings_and_diags_without_early_return() {
        // A feature with one bindable step and one unbound step: collect-all must
        // return the bound step's binding AND the unbound diagnostic together.
        let packs = crate::pack::load(
            &[crate::pack::PackSource {
                name: "packs/p.yaml".to_owned(),
                text: std::sync::Arc::from(
                    "macros:\n  greet:\n    params: [who]\n    match: \"I greet {who}\"\n    steps:\n      - hurl: |\n          GET http://x\n",
                ),
            }],
            &crate::pack::FragmentCorpus::empty(),
            KINDS,
        )
        .unwrap();
        let file = crate::feature::parse(
            "f.feature",
            "Feature: F\n  Scenario: S\n    When I greet Sam\n    And I xyzzy\n",
        )
        .unwrap();

        let (scenarios, diags) = bind_collect(&file, &packs);
        // One scenario, with the bound step surviving.
        let bound_step_count: usize = scenarios.iter().map(|s| s.steps.len()).sum();
        assert_eq!(bound_step_count, 1, "the bindable step must survive");
        assert_eq!(scenarios[0].steps[0].macro_name, "greet");
        // The unbound step surfaces its diagnostic rather than aborting the feature.
        assert!(diags.iter().any(|d| d.code == "proef::bind::unbound_step"));
    }
}
