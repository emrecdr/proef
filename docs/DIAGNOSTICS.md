# Diagnostic codes — the greppable index

Every proef diagnostic carries a stable code (`proef::<area>::<name>`), printed
above the rendered message. Codes are a contract: they never change meaning,
and searching this file (or `tests/errors/`) for a code you hit is the fastest
route to the cause. The **corpus** column marks codes with a seeded broken
example under `tests/errors/<area>__<name>/` — dry-running one shows the exact
rendered output.

Severity is **error** (fails validation/exit 2) unless marked *warning*.

## `proef::feature::*` — feature-file parsing and expansion

| Code | Meaning | Corpus |
|---|---|---|
| `feature::parse` | The file is not valid Gherkin (parser error, located) | ✓ |
| `feature::empty_file` | The file is empty — a `Feature:` header is required | |
| `feature::no_examples` | A `Scenario Outline` has no Examples rows | |
| `feature::ragged_examples` | An Examples row's cell count differs from the header | |
| `feature::bad_examples_header` | Duplicate or empty Examples column name | |
| `feature::unknown_placeholder` | An outline step uses `<name>` not present in the header | ✓ |
| `feature::empty_scenario` | A scenario has no steps (background included) | ✓ |

## `proef::pack::*` — macro-pack loading and validation

| Code | Meaning | Corpus |
|---|---|---|
| `pack::yaml` | The pack file is not valid YAML | ✓ |
| `pack::duplicate_macro` | Two macros share a name | ✓ |
| `pack::empty_macro` | A macro has neither `steps:` nor `expect:` | |
| `pack::steps_and_expect` | A macro has both `steps:` and `expect:` | |
| `pack::empty_expect` | An `expect:` item asserts nothing | ✓ |
| `pack::empty_step` | A step has no payload and no `use:` | |
| `pack::multiple_payloads` | A step has more than one payload key | |
| `pack::unknown_step_kind` | The payload key names no registered engine | ✓ |
| `pack::invalid_hurl` | The payload does not parse as hurl (incl. zero-entry payloads) | ✓ |
| `pack::payload_invalid` | A structured payload fails the engine's validator | ✓ |
| `pack::bad_reference` | A `${…}` reference in the payload cannot resolve at probe time | |
| `pack::retry_not_finite` | `retry`/`repeat` is `-1`, `0`, or above the 10000 cap | ✓ |
| `pack::delay_unbounded` | `delay` exceeds the 1-hour cap | |
| `pack::option_declared_twice` | `retry`/`delay` set both in the block's `[Options]` and as the step's own key, or a variable both supplied by a fragment's `[Options] variable:` and given by a `bind:` | ✓ |
| `pack::pattern_braces` | Unbalanced `{`/`}` in a `match:` pattern | |
| `pack::pattern_empty_capture` | `{}` with no capture name | |
| `pack::pattern_no_anchor` | A pattern with no literal word (capture-only) | ✓ |
| `pack::pattern_unknown_capture` | A `{capture}` that is not a declared param | ✓ |
| `pack::pattern_duplicate_capture` | The same `{capture}` written twice in one pattern | ✓ |
| `pack::adjacent_captures` | `{a} {b}` with no literal between captures | ✓ |
| `pack::default_not_param` | A `defaults:` key that is not a declared param | ✓ |
| `pack::bad_save_target` | `saveAs:` target other than `global` | |
| `pack::unknown_ref` | `ref:` names no loaded fragment (suggests the closest) | ✓ |
| `pack::duplicate_fragment` | Two fragment files declare the same `# @proef` name | |
| `pack::bad_annotation` | A fragment file the engine's parser could not read, an annotation it could not attach, or a name containing `#` (which could never be referenced) | |
| `pack::unreadable_fragment_file` | A fragment file that could not be read at all (encoding, permissions) — its siblings still load, and it is silent until something `ref:`s the corpus | |
| `pack::oversized_fragment_file` | A fragment file over 8 MiB — skipped unread (the size comes from the directory entry), siblings still load | |
| `pack::fragment_corpus_too_large` | The corpus as a whole passed 64 MiB — the read stops, naming the file it stopped at | |
| `pack::body_form_conflict` | A step is both `ref:` and a payload (or `use:`) | ✓ |
| `pack::bind_without_ref` | `bind:` with no `ref:` to read it — on a step (an inline block takes `${…}` instead), or on a macro whose steps have none (a `use:` target resolves its own) | ✓ |
| `pack::unread_bind_key` | a `bind:` *key* no fragment in that scope reads (did-you-mean over the readable names) — the finer half of `bind_without_ref`, and the one a typo produces | |
| `pack::unknown_use` | `use:` names no known macro | ✓ |
| `pack::use_cycle` | `use:` composition forms a cycle | ✓ |
| `pack::use_too_deep` | `use:` nesting exceeds depth 32 | |
| `pack::use_with_modifiers` | A `use:` step carries modifiers that belong on the target | |
| `pack::use_with_payload` | A step is both `use:` and a payload | |
| `pack::with_without_use` | `with:` on a step that has no `use:` | |
| `pack::missing_use_param` | The `use:` target requires a param `with:` does not supply | ✓ |
| `pack::unknown_with_key` | A `with:` key the target macro does not declare | ✓ |

## `proef::bind::*` — matching prose to macros

| Code | Meaning | Corpus |
|---|---|---|
| `bind::unbound_step` | No macro pattern matches the sentence (suggests the closest, plus a paste-ready macro stub) | ✓ |
| `bind::ambiguous_step` | More than one macro matches (all candidates listed) | ✓ |
| `bind::missing_param` | A required param has no capture, table value, or default | ✓ |
| `bind::bad_table` | A data table has an unusable shape | |
| `bind::unknown_table_key` | A table key that is not a declared param | |
| `bind::table_conflict` | A table value collides with a pattern capture | ✓ |
| `bind::docstring_unused` | A docstring the macro never references (*warning*) | |

## `proef::lower::*` — lowering to engine batches

| Code | Meaning | Corpus |
|---|---|---|
| `lower::then_before_when` | An `expect:` step with no previous request to attach to | ✓ |
| `lower::bad_status` | `expect: status:` is not an HTTP status number | |
| `lower::kind_unrouted` | Internal safety net: a lowered step's kind maps to no engine (registry drift — unreachable through the CLI, which is why it has no corpus case) | |
| `lower::expansion_too_deep` | Macro expansion exceeded depth 32 at run time | |
| `lower::unbound_placeholder` | A `{{variable}}` nothing supplies — read by the fragment, or inside a `bind:` value (as the engine's parser reads it: a hurl *function* like `{{newUuid}}` is not a variable) — with no `bind:` in scope, no earlier capture, no fragment-own `[Options] variable:`, no earlier-sorting sibling literal, and no secret of that name | |
| `lower::bind_shadows_capture` | A literal `bind:` re-assigns a name an earlier step captured, so the bound value silently wins from that entry on (*warning*) | |
| `lower::multiline_bind` | A `bind:` value that resolves to a line break or control character (tab excepted) — a hurl `[Options] variable:` is a single-line scalar; the inline `hurl: \|` form is what splices a multi-line body | |
| `lower::secret_in_composite_bind` | A `bind:` value mixes `${secret:…}` into a larger string — bind the secret alone and put the surrounding text in the fragment | |
| `lower::dry_run_unknown` | A runtime-only global under `--dry-run` (*warning*) | |

## `proef::resolve::*` — `${…}` variable resolution

| Code | Meaning | Corpus |
|---|---|---|
| `resolve::unknown_variable` | `${name}` found in no scope (suggests the closest) | ✓ |
| `resolve::missing_env` | `${env:NAME}` unset and no `:-default` given | |
| `resolve::missing_config_var` | `${url:key}` / `${vars:key}` defined in neither `proef.toml` nor the active `[env.<name>]` (suggests the closest) | ✓ |
| `resolve::missing_global` | `${global:key}` absent from the World (strict mode) | |
| `resolve::unknown_namespace` | `${ns:…}` with an unknown namespace | |
| `resolve::unknown_run_field` | `${run:…}` other than `${run:id}` | |
| `resolve::fake_unknown` | `${fake:kind}` names no generator (suggests the closest) | |
| `resolve::empty_reference` | An empty `${}` | |
| `resolve::depth_exceeded` | Resolution passed depth 8 — a reference cycle | |

## `proef::emit::*` — artifact emission

| Code | Meaning | Corpus |
|---|---|---|
| `emit::invalid_artifact` | The emitted artifact does not parse with the engine's parser | |

## `proef::run::*` — preparing a scenario to execute

| Code | Meaning | Corpus |
|---|---|---|
| `run::asset_unstageable` | A `file,…;` asset could not be staged into the scenario's asset root: absent beside the source that names it, named by a path proef will not follow, or claimed by two sources at once | |

## `proef::config::*` — `proef.toml` loading

| Code | Meaning | Corpus |
|---|---|---|
| `config::toml` | The file is not valid TOML for the config schema (located — the caret sits on toml's own error span) | |
| `config::unreadable` | The file (or an explicit `--config` path) cannot be read | |

## `proef::source::*` — source access (LSP whole-suite analysis)

| Code | Meaning | Corpus |
|---|---|---|
| `source::unreadable` | A discovered feature or pack source could not be read (surfaced by `analyze_suite`; the CLI treats an unreadable file as a system fault instead) | |

## Coverage note

The fragment-file codes (`pack::duplicate_fragment`, `pack::bad_annotation`,
`pack::unreadable_fragment_file`, `pack::unread_bind_key`,
`lower::unbound_placeholder`, `lower::multiline_bind`,
`lower::secret_in_composite_bind`, `run::asset_unstageable`) are covered in
`crates/proef-cli/tests/fragments.rs` rather than `tests/errors/`: they need a
`[run] fragments` root, and the seeded corpus is deliberately config-independent.

The `config::*` codes are covered in `crates/proef-cli/tests/cli.rs` for the
same reason: a broken `proef.toml` cannot live in the config-independent
seeded corpus.

30 of the 76 codes carry a seeded corpus case today; the corpus guard asserts
a minimum, not parity. When you add a diagnostic, add its code here and prefer
seeding a `tests/errors/<area>__<name>/` case alongside it.

Every row here is a code some code path actually emits. That was not free: this
file used to carry a `pack::load` row for a defensive case that never had a code
(a non-diagnostic core failure while loading packs renders as a bare `error:`),
so a reader who grepped for it found nothing and had no way to tell the index was
wrong. A row nothing emits is worse than a missing row.
