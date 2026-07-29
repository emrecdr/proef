//! Author-time `${…}` variable resolution (ADR-0005, TECH-SPEC §8).
//!
//! `${…}` is the **author-time** tier, resolved during lowering — recursively
//! (substituted values may themselves contain `${…}`, spike-verified), with a
//! depth cap of [`MAX_DEPTH`]. `{{…}}` is hurl's **run-time** tier and passes
//! through untouched. `$${` escapes to a literal `${` (applied after the final
//! pass, so escaped text is never re-resolved).
//!
//! Reference forms: `${param}` (scope lookup: step args > macro defaults >
//! feature directives) · `${env:NAME}` / `${env:NAME:-default}`
//! (injected snapshot — core reads no environment) · `${run:id}` (injected) ·
//! `${global:key}` (World read at lower time) · `${secret:NAME}` (emits the
//! `{{NAME}}` run-time placeholder and records the name — values never enter
//! lowered text) · `${fake:kind}` (deterministic synthetic data).

use std::collections::{BTreeMap, BTreeSet};

use crate::world::World;

/// Maximum resolution passes before assuming a reference cycle (TECH-SPEC §4.4).
pub const MAX_DEPTH: usize = 8;

/// How strictly to treat values that only exist at run time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveMode {
    /// Execution: a missing `${global:key}` is an error.
    Strict,
    /// `--dry-run`: runtime-populated globals cannot be known — substitute an
    /// empty string and record a warning instead of failing.
    DryRun,
    /// Pack-load probe instantiation (validation pass 7): any reference that
    /// merely *might* resolve later (unknown vars, env, globals, fakes)
    /// substitutes the placeholder `probe` — only statically-wrong syntax
    /// (empty reference, unknown namespace, unknown run field) still errors.
    Probe,
}

/// Everything a resolution pass may read. All values are injected — resolution
/// itself is pure (core purity).
#[derive(Debug, Clone, Copy)]
pub struct ResolveCtx<'a> {
    /// Step arguments (captures + data table + `with:`), highest precedence.
    pub args: &'a BTreeMap<String, String>,
    /// Macro `defaults:`.
    pub defaults: &'a BTreeMap<String, String>,
    /// Feature `# key: value` directives.
    pub directives: &'a BTreeMap<String, String>,
    /// Injected environment snapshot.
    pub env: &'a BTreeMap<String, String>,
    /// Injected run identifier (`${run:id}`).
    pub run_id: &'a str,
    /// World, for `${global:key}` reads at lower time.
    pub world: &'a World,
    /// Strict (execution) or dry-run behavior.
    pub mode: ResolveMode,
}

/// A successful resolution: the final text plus what it referenced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolution {
    /// The resolved text (`{{…}}` untouched, escapes applied).
    pub text: String,
    /// Secret names referenced via `${secret:NAME}` (values never appear).
    pub secrets: BTreeSet<String>,
    /// Global keys read via `${global:key}` (drives `.vars` emission, ADR-0010).
    pub globals: BTreeSet<String>,
    /// `${fake:*}` values generated so far (occurrence indexing).
    pub fakes: usize,
    /// Dry-run soft findings (e.g. a runtime-only global).
    pub warnings: Vec<String>,
}

/// Why a template failed to resolve. Codes are stable diagnostic identifiers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// A plain `${name}` found in no scope.
    #[error("unknown variable `${{{name}}}`{}", suggestion.as_ref().map(|s| format!(" — did you mean `{s}`?")).unwrap_or_default())]
    UnknownVariable {
        /// The unresolved name.
        name: String,
        /// Closest known name, when one is near.
        suggestion: Option<String>,
    },
    /// `${env:NAME}` without a default, and NAME is not in the snapshot.
    #[error(
        "environment variable `{name}` is not set (use `${{env:{name}:-default}}` for a fallback)"
    )]
    MissingEnv {
        /// The missing environment variable.
        name: String,
    },
    /// `${global:key}` missing from the World (strict mode only).
    #[error("global `{key}` is not set in the World")]
    MissingGlobal {
        /// The missing global key.
        key: String,
    },
    /// `${ns:…}` with an unrecognized namespace.
    #[error("unknown variable namespace `{namespace}:` (known: env, run, global, secret, fake)")]
    UnknownNamespace {
        /// The namespace as written.
        namespace: String,
    },
    /// `${run:…}` with something other than `id`.
    #[error("unknown run field `{field}` (only `${{run:id}}` exists)")]
    UnknownRunField {
        /// The field as written.
        field: String,
    },
    /// `${fake:…}` names no known generator (statically rejected).
    #[error("unknown fake generator `{kind}`{}", suggestion.as_ref().map(|s| format!(" — did you mean `{s}`?")).unwrap_or_default())]
    FakeUnknown {
        /// The requested generator kind.
        kind: String,
        /// Closest known generator, when one is near.
        suggestion: Option<String>,
    },
    /// An empty reference `${}`.
    #[error("empty variable reference `${{}}`")]
    EmptyReference,
    /// Still-unresolved `${…}` after [`MAX_DEPTH`] passes — a reference cycle.
    #[error(
        "variable resolution exceeded depth {MAX_DEPTH} (reference cycle through `${{{name}}}`?)"
    )]
    DepthExceeded {
        /// A variable still unresolved when the cap was hit.
        name: String,
    },
}

impl ResolveError {
    /// The stable diagnostic code for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownVariable { .. } => "proef::resolve::unknown_variable",
            Self::MissingEnv { .. } => "proef::resolve::missing_env",
            Self::MissingGlobal { .. } => "proef::resolve::missing_global",
            Self::UnknownNamespace { .. } => "proef::resolve::unknown_namespace",
            Self::UnknownRunField { .. } => "proef::resolve::unknown_run_field",
            Self::FakeUnknown { .. } => "proef::resolve::fake_unknown",
            Self::EmptyReference => "proef::resolve::empty_reference",
            Self::DepthExceeded { .. } => "proef::resolve::depth_exceeded",
        }
    }
}

/// Resolve every `${…}` in `text` (recursively, ≤ [`MAX_DEPTH`] passes), leave
/// `{{…}}` untouched, then apply `$${` escapes. Pure and total.
pub fn resolve(text: &str, ctx: &ResolveCtx<'_>) -> Result<Resolution, ResolveError> {
    let mut resolution = Resolution::default();
    let mut current = text.to_owned();

    for _ in 0..MAX_DEPTH {
        let (next, substituted) = resolve_pass(&current, ctx, &mut resolution)?;
        current = next;
        if !substituted {
            resolution.text = unescape(&current);
            return Ok(resolution);
        }
    }

    if let Some((name, _, _)) = first_reference(&current) {
        Err(ResolveError::DepthExceeded {
            name: name.to_owned(),
        })
    } else {
        resolution.text = unescape(&current);
        Ok(resolution)
    }
}

/// One left-to-right substitution pass. Returns the new text and whether any
/// reference was substituted.
fn resolve_pass(
    text: &str,
    ctx: &ResolveCtx<'_>,
    resolution: &mut Resolution,
) -> Result<(String, bool), ResolveError> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut substituted = false;

    while let Some((name, start, end)) = first_reference(rest) {
        out.push_str(&rest[..start]);
        let value = lookup(name, ctx, resolution)?;
        out.push_str(&value);
        substituted = true;
        rest = &rest[end..];
    }
    out.push_str(rest);
    Ok((out, substituted))
}

/// Find the first live `${…}` reference, skipping `$${` escapes. Returns
/// `(name, start_of_ref, end_after_brace)` in byte offsets.
fn first_reference(text: &str) -> Option<(&str, usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            // `$${` — escaped: skip the whole escape marker.
            if text[i..].starts_with("$${") {
                i += 3;
                continue;
            }
            if text[i..].starts_with("${") {
                let after = &text[i + 2..];
                if let Some(close) = after.find('}') {
                    let name = &after[..close];
                    return Some((name, i, i + 2 + close + 1));
                }
                // Unclosed `${` — treat as literal text.
                return None;
            }
        }
        i += 1;
    }
    None
}

/// Resolve one reference name to its substitution value.
fn lookup(
    name: &str,
    ctx: &ResolveCtx<'_>,
    resolution: &mut Resolution,
) -> Result<String, ResolveError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ResolveError::EmptyReference);
    }

    if let Some((namespace, arg)) = name.split_once(':') {
        return match namespace {
            "env" => {
                let (var, default) = match arg.split_once(":-") {
                    Some((var, default)) => (var, Some(default)),
                    None => (arg, None),
                };
                match ctx.env.get(var) {
                    Some(value) => Ok(value.clone()),
                    None => match default {
                        Some(default) => Ok(default.to_owned()),
                        None => probe_or(
                            ResolveError::MissingEnv {
                                name: var.to_owned(),
                            },
                            ctx.mode,
                        ),
                    },
                }
            }
            "run" => {
                if arg == "id" {
                    Ok(ctx.run_id.to_owned())
                } else {
                    Err(ResolveError::UnknownRunField {
                        field: arg.to_owned(),
                    })
                }
            }
            "global" => {
                resolution.globals.insert(arg.to_owned());
                match ctx.world.get(arg) {
                    Some(value) => Ok(value.to_string()),
                    None => match ctx.mode {
                        ResolveMode::Strict => Err(ResolveError::MissingGlobal {
                            key: arg.to_owned(),
                        }),
                        ResolveMode::DryRun => {
                            resolution.warnings.push(format!(
                                "`${{global:{arg}}}` is not set yet — it may be populated at run time"
                            ));
                            Ok(String::new())
                        }
                        ResolveMode::Probe => Ok("probe".to_owned()),
                    },
                }
            }
            "secret" => {
                resolution.secrets.insert(arg.to_owned());
                // The run-time placeholder; the engine injects the value via
                // `insert_secret` at run time — lowered text never carries it.
                Ok(format!("{{{{{arg}}}}}"))
            }
            "fake" => {
                // Unknown generators are statically wrong — they error in every
                // mode (incl. Probe: the pack lint catches typos at load).
                if !crate::fake::is_known_generator(arg) {
                    return Err(ResolveError::FakeUnknown {
                        kind: arg.to_owned(),
                        suggestion: crate::matcher::closest(
                            arg,
                            crate::fake::GENERATORS.iter().copied(),
                        )
                        .map(ToOwned::to_owned),
                    });
                }
                let occurrence = resolution.fakes;
                resolution.fakes += 1;
                crate::fake::generate(ctx.run_id, occurrence, arg).ok_or_else(|| {
                    ResolveError::FakeUnknown {
                        kind: arg.to_owned(),
                        suggestion: None,
                    }
                })
            }
            other => Err(ResolveError::UnknownNamespace {
                namespace: other.to_owned(),
            }),
        };
    }

    // Plain name: args > defaults > directives (TECH-SPEC §8).
    for scope in [ctx.args, ctx.defaults, ctx.directives] {
        if let Some(value) = scope.get(name) {
            return Ok(value.clone());
        }
    }
    let known = ctx
        .args
        .keys()
        .chain(ctx.defaults.keys())
        .chain(ctx.directives.keys());
    probe_or(
        ResolveError::UnknownVariable {
            name: name.to_owned(),
            suggestion: crate::matcher::closest(name, known.map(String::as_str))
                .map(ToOwned::to_owned),
        },
        ctx.mode,
    )
}

/// In [`ResolveMode::Probe`], soften might-resolve-later failures to the
/// `probe` placeholder; otherwise propagate the error.
fn probe_or(err: ResolveError, mode: ResolveMode) -> Result<String, ResolveError> {
    if mode == ResolveMode::Probe {
        Ok("probe".to_owned())
    } else {
        Err(err)
    }
}

/// Apply `$${` → `${` escapes (after the final pass — never re-resolved).
fn unescape(text: &str) -> String {
    text.replace("$${", "${")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::world::{GlobalStore, Value};

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    struct Fixture {
        args: BTreeMap<String, String>,
        defaults: BTreeMap<String, String>,
        directives: BTreeMap<String, String>,
        env: BTreeMap<String, String>,
        world: World,
    }

    impl Fixture {
        fn new() -> Self {
            let mut store = GlobalStore::new();
            store.insert("clientId", Value::String("c-42".into()));
            Self {
                args: map(&[("lastName", "Bakker-${run:id}")]),
                defaults: map(&[("index", "clients")]),
                directives: map(&[("baseURL", "http://fixture.local")]),
                env: map(&[("HOME", "/home/test")]),
                world: World::new(store),
            }
        }

        fn ctx(&self, mode: ResolveMode) -> ResolveCtx<'_> {
            ResolveCtx {
                args: &self.args,
                defaults: &self.defaults,
                directives: &self.directives,
                env: &self.env,
                run_id: "run-0001",
                world: &self.world,
                mode,
            }
        }
    }

    #[test]
    fn scope_precedence_and_recursion() {
        let f = Fixture::new();
        // The captured arg itself contains ${run:id} — the spike-verified case.
        let r = resolve(
            "GET ${baseURL}/search?q=${lastName}",
            &f.ctx(ResolveMode::Strict),
        )
        .unwrap();
        assert_eq!(r.text, "GET http://fixture.local/search?q=Bakker-run-0001");
    }

    #[test]
    fn runtime_tier_passes_through() {
        let f = Fixture::new();
        let r = resolve(
            "Authorization: Bearer {{token}}",
            &f.ctx(ResolveMode::Strict),
        )
        .unwrap();
        assert_eq!(r.text, "Authorization: Bearer {{token}}");
    }

    #[test]
    fn escape_round_trips() {
        let f = Fixture::new();
        let r = resolve("literal $${notavar} stays", &f.ctx(ResolveMode::Strict)).unwrap();
        assert_eq!(r.text, "literal ${notavar} stays");
    }

    #[test]
    fn env_defaults_apply() {
        let f = Fixture::new();
        let ctx = f.ctx(ResolveMode::Strict);
        assert_eq!(resolve("${env:HOME}", &ctx).unwrap().text, "/home/test");
        assert_eq!(
            resolve("${env:NOPE:-fallback}", &ctx).unwrap().text,
            "fallback"
        );
        let err = resolve("${env:NOPE}", &ctx).unwrap_err();
        assert_eq!(err.code(), "proef::resolve::missing_env");
    }

    #[test]
    fn secrets_become_runtime_placeholders_and_are_recorded() {
        let f = Fixture::new();
        let r = resolve("Bearer ${secret:apiToken}", &f.ctx(ResolveMode::Strict)).unwrap();
        assert_eq!(r.text, "Bearer {{apiToken}}");
        assert!(r.secrets.contains("apiToken"));
    }

    #[test]
    fn globals_read_from_the_world() {
        let f = Fixture::new();
        let r = resolve("id=${global:clientId}", &f.ctx(ResolveMode::Strict)).unwrap();
        assert_eq!(r.text, "id=c-42");
    }

    #[test]
    fn missing_global_is_strict_error_but_dry_run_warning() {
        let f = Fixture::new();
        let err = resolve("${global:nope}", &f.ctx(ResolveMode::Strict)).unwrap_err();
        assert_eq!(err.code(), "proef::resolve::missing_global");

        let r = resolve("${global:nope}", &f.ctx(ResolveMode::DryRun)).unwrap();
        assert_eq!(r.text, "");
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn unknown_variable_suggests_the_closest_name() {
        let f = Fixture::new();
        let err = resolve("${lastNam}", &f.ctx(ResolveMode::Strict)).unwrap_err();
        let ResolveError::UnknownVariable { suggestion, .. } = &err else {
            panic!("wrong variant: {err:?}");
        };
        assert_eq!(suggestion.as_deref(), Some("lastName"));
    }

    #[test]
    fn reference_cycles_hit_the_depth_cap() {
        let mut f = Fixture::new();
        f.args = map(&[("a", "${b}"), ("b", "${a}")]);
        let err = resolve("${a}", &f.ctx(ResolveMode::Strict)).unwrap_err();
        assert_eq!(err.code(), "proef::resolve::depth_exceeded");
    }

    #[test]
    fn fakes_generate_deterministically_and_reject_typos() {
        let f = Fixture::new();
        let once = resolve(
            "${fake:firstName} ${fake:firstName}",
            &f.ctx(ResolveMode::Strict),
        )
        .unwrap()
        .text;
        let twice = resolve(
            "${fake:firstName} ${fake:firstName}",
            &f.ctx(ResolveMode::Strict),
        )
        .unwrap()
        .text;
        assert_eq!(once, twice, "deterministic per run id");
        assert!(!once.trim().is_empty());

        let err = resolve("${fake:firstNam}", &f.ctx(ResolveMode::Strict)).unwrap_err();
        assert_eq!(err.code(), "proef::resolve::fake_unknown");
        assert!(err.to_string().contains("firstName"), "{err}");
        // Typos are static: even Probe mode rejects them (pack lint).
        assert!(resolve("${fake:firstNam}", &f.ctx(ResolveMode::Probe)).is_err());
    }

    #[test]
    fn unclosed_reference_is_literal() {
        let f = Fixture::new();
        let r = resolve("half ${open and done", &f.ctx(ResolveMode::Strict)).unwrap();
        assert_eq!(r.text, "half ${open and done");
    }

    mod properties {
        #![allow(clippy::ignored_unit_patterns)]

        use super::*;
        use proptest::prelude::*;

        fn empty_ctx_fixture() -> Fixture {
            let mut f = Fixture::new();
            f.args = BTreeMap::new();
            f.defaults = BTreeMap::new();
            f.directives = BTreeMap::new();
            f
        }

        proptest! {
            /// Total on arbitrary input: resolve never panics (mirrors the fuzz target).
            #[test]
            fn resolver_never_panics(text in ".{0,200}") {
                let f = Fixture::new();
                let _ = resolve(&text, &f.ctx(ResolveMode::DryRun));
            }

            /// `$${…}` escape round-trip on arbitrary brace-free inner text.
            #[test]
            fn escape_round_trip(inner in "[^{}$]{0,40}") {
                let f = empty_ctx_fixture();
                let text = format!("$${{{inner}}}");
                let resolved = resolve(&text, &f.ctx(ResolveMode::Strict)).unwrap();
                prop_assert_eq!(resolved.text, format!("${{{inner}}}"));
            }

            /// Once fully resolved (and escape-free), resolution is idempotent.
            #[test]
            fn idempotent_after_fixpoint(text in "[^$]{0,120}") {
                let f = empty_ctx_fixture();
                let ctx = f.ctx(ResolveMode::Strict);
                let once = resolve(&text, &ctx).unwrap();
                let twice = resolve(&once.text, &ctx).unwrap();
                prop_assert_eq!(&once.text, &twice.text);
            }

            /// The depth cap always terminates resolution, whatever the scopes hold.
            #[test]
            fn always_terminates(
                keys in proptest::collection::vec("[a-c]{1}", 1..3),
                text in "[a-c${}]{0,60}",
            ) {
                let mut f = empty_ctx_fixture();
                // Self-referential scopes: worst case for the pass loop.
                f.args = keys.iter().map(|k| (k.clone(), format!("${{{k}}}"))).collect();
                let _ = resolve(&text, &f.ctx(ResolveMode::DryRun));
            }
        }
    }
}
