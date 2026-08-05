# proef v0.5.1 — LSP correctness fix pass (design)

**Goal:** Fix the three confirmed P0 LSP blockers shipped in v0.5.0, plus two
validated correctness items and the proef-lsp documentation debt, so the language
server is genuinely usable in a real editor session.

**Architecture:** Bug-fix pass over the existing crates — no new modules, no new
deps, no architectural change. The core stays sans-IO; the one core change is a
`pub(crate)` extraction that leaves the public API byte-identical. Each fix ships
with the test that would have caught it (the shipped blockers all slipped because
every LSP test used `Connection::memory()` + synthetic clean `FakeDisk` suites).

**Tech stack:** Rust 2024; `lsp-server 0.7.9` / `lsp-types 0.97.0` (both pinned,
unchanged); `serde_norway`; existing `assert_cmd`/`Connection::memory` test harnesses.

**Branch:** `feat/lsp-v051-fixes` off `main` (5c36a5e = v0.5.0). Independent of the
open PR #4 (`feat/lsp-goto-def-gaps`); they touch different code.

---

## Scope

**In:**
1. §2.1 — server process leak (stdio never exits) → `drop(connection)` before `join()`.
2. §2.2 — malformed request kills the server → reply `InvalidParams` + continue.
3. §2.3b degradation — one broken pack zeroes the whole analysis → collect-all pack load.
4. §2.3a root-at-suite — LSP walks the whole repo from cwd → scope to the configured suite.
5. §2.3b overlay-URI — unsaved edits silently ignored for sub-delim paths → key overlay by name.
6. Docs sweep — EDITORS.md currency + proef-lsp into README/docs/README/TECH-SPEC §10+§14/RELEASING.
7. Coverage — a real stdio lifecycle test, a malformed-params test, a degradation test.

**Out (validated as out-of-scope, with reasons):**
- Built-in-macro go-to-def "null": **not a bug.** `name_to_url("builtin:core")` → `None`
  → `goto` returns a clean null; a compiled-in macro has no file location. No change.
- EDITORS.md go-to-definition claim (lines 100-102, "lands on the macro name, not a
  `match:` line"): **correct on this branch.** v0.5.0 targets the name-key (`def_span`);
  the claim only drifts once PR #4 lands, so that fix belongs to PR #4, not here.
- Config hot-reload: deferred; the startup-snapshot limitation (EDITORS.md 95-99) stays.
- All P1 CLI-correctness items (diff keying, fail-on-regression on truncated runs,
  `[run] setup`=dir double-run, EPIPE in the diagnostic renderer, exit-130 docs): the
  **follow-on pass**, not v0.5.1.

---

## Verified facts (assumptions validated against real source before design)

Per the standing directive, every fix mechanism was confirmed against the actual
pinned crate source and proef code — not assumed.

- **§2.1 leak — confirmed + fix correct.** `run()` (server.rs:95-119) owns `connection`
  and calls `io_threads.join()` (118) with it still in scope. In `lsp-server 0.7.9`,
  `Connection.sender` is the *sole* `Sender` of the writer channel
  (`stdio.rs:16-27`, `lib.rs:41-42`); the writer thread's `writer_receiver.into_iter()`
  ends only when that sender drops; `IoThreads::join()` joins reader→dropper→writer.
  Holding `connection` during `join()` deadlocks forever; `drop(connection)` first
  releases all three joins. → `drop(connection)` before `join()` is exactly right.
- **§2.2 crash — confirmed reachable + fix available.** The three feature arms
  (server.rs:267/289/311) do `from_value(...).map_err(ServerError::Protocol)?`; the `?`
  unwinds through `dispatch_request(...)?` (174) → out of `main_loop` → `run` returns
  `Err` → `ExitCode::SystemError`. `lsp_types::Uri` deser parses through `fluent_uri`
  (`uri.rs:18-28`) so an RFC-3986-invalid URI (e.g. a raw space) *fails* to deserialize;
  `Position.line/.character` are `u32` so a negative line *fails*. `ErrorCode::InvalidParams`
  = -32602 and `Response::new_err(id, i32, String)` both exist (`msg.rs:101-109,203`) —
  the fix mirrors the existing MethodNotFound arm (server.rs:333-341).
- **§2.3b degradation — confirmed cheap.** `pack::load` (mod.rs:254-317) *already* builds
  a `PackSet` from every pack that parses+normalizes and collects all diagnostics; a
  broken pack is merely excluded. The *only* fail-fast is the final gate (309-313): "any
  Error diag → `Err`, discard the set." Extracting that discard is the whole fix.
- **§2.3a root — confirmed the real gap.** `lsp.rs:45` roots at `current_dir()`; the repo's
  `proef.toml` has no `[run] suite`, so the `tests/` convention (`resolve_suite_path`,
  main.rs:228-246) resolves to `tests/`, which *contains* the broken `tests/errors/`
  corpus. So root-at-suite alone is insufficient — degradation is load-bearing; root-at-suite
  is complementary scoping (out of `target/`, `docs/`).
- **§2.3b overlay — confirmed a real bug.** `lsp_types::Uri` Eq/Hash compare the *raw*
  `as_str()` string with no normalization (`uri.rs:68-80`; `fluent_uri` has no Eq at all);
  `(` is legal *bare* in a URI path (fluent_uri `SUB_DELIMS`), so `file:///a(b` (client)
  ≠ `file:///a%28b` (`name_to_url`) → the `HashMap<Uri,_>` overlay lookup misses. EDITORS.md
  113-116 already documents this as a known v1 limitation.

---

## Design

### Fix 1 — §2.1 process leak (proef-lsp/src/server.rs)

In `run()`, after `main_loop` returns on the success path, drop the connection before
joining the transport:

```rust
main_loop(&connection, &cfg)?;
// Release the writer Sender so lsp-server's stdio writer thread can finish;
// IoThreads::join() waits on it, and it only ends when this Sender drops.
drop(connection);
if let Some(threads) = io_threads {
    threads.join().map_err(ServerError::Io)?;
}
Ok(())
```

On the `?` error path, `run` returns `Err` immediately; `connection` and `io_threads`
drop at scope exit and `io_threads` drop does *not* join (it detaches), so there is no
hang. The drop-before-join only matters on the success path, where we explicitly join.

### Fix 2 — §2.2 malformed request (proef-lsp/src/server.rs)

Add one small helper and route all three feature arms through it:

```rust
/// Deserialize a request's params, or produce an InvalidParams error Response
/// to send back so a malformed request is answered — never fatal to the loop.
fn parse_params<P: serde::de::DeserializeOwned>(
    req: &lsp_server::Request,
) -> Result<P, lsp_server::Response> {
    serde_json::from_value(req.params.clone()).map_err(|e| {
        lsp_server::Response::new_err(
            req.id.clone(),
            lsp_server::ErrorCode::InvalidParams as i32,
            format!("invalid params for {}: {e}", req.method),
        )
    })
}
```

Each arm becomes:

```rust
let params: lsp_types::GotoDefinitionParams = match parse_params(req) {
    Ok(p) => p,
    Err(resp) => {
        return connection
            .sender
            .send(Message::Response(resp))
            .map_err(|e| ServerError::Protocol(e.to_string()));
    }
};
```

The server replies `InvalidParams` and the loop continues — matching how the
unknown-method arm already handles the unhandled case. A `send` failure (the transport
is genuinely gone) still legitimately surfaces as `Protocol`, as it does today.

### Fix 3 — §2.3b degradation (proef-core/src/pack/mod.rs + analyze.rs)

Extract the collect-all body; `load` becomes the fail-fast wrapper.

```rust
// pack/mod.rs — pub(crate): only the in-core collect-all analyzer needs it, so
// this adds NO public API surface.
pub(crate) fn load_collecting(
    sources: &[PackSource],
    kinds: &[StepKindSpec],
) -> (PackSet, Vec<Diag>) {
    // ... the current body of `load`, minus the final Result gate ...
    (set, diags)
}

pub fn load(sources: &[PackSource], kinds: &[StepKindSpec]) -> Result<PackSet, FrontError> {
    let (set, diags) = load_collecting(sources, kinds);
    if diags.iter().any(|d| d.severity == crate::diag::Severity::Error) {
        Err(FrontError::Diagnostics(diags))
    } else {
        Ok(set)
    }
}
```

In `analyze_suite` (analyze.rs), replace the `pack::load` match + early `return out`
(112-121) with `load_collecting`: push all pack diagnostics per source name, build the
`MacroRef` vocabulary from the (partial) set, and continue into feature binding. The
built-in packs always parse, so the `expect*` family always survives; a broken user pack
degrades to "its macros are missing" (its steps become `unbound`), never a dark suite.
The doc comment on `analyze_suite` is updated: broken packs no longer short-circuit.

### Fix 4 — §2.3a root-at-suite (proef-cli/src/config.rs + main.rs + lsp.rs)

One shared convention, no divergent copy. Add to `ProjectConfig`:

```rust
/// The default suite directory: `[run] suite` if set, else the `tests/`
/// convention when it exists on disk, else None. The one place both the suite
/// commands and the LSP derive "which directory is the suite".
pub fn default_suite_path(&self) -> Option<PathBuf> {
    if let Some(suite) = self.suite() {
        return Some(PathBuf::from(suite));
    }
    let convention = PathBuf::from("tests");
    convention.is_dir().then_some(convention)
}
```

`resolve_suite_path` (main.rs) delegates to it: `path.or_else(|| config.default_suite_path())`
then the existing error-on-none. `lsp.rs` roots best-effort at the suite, made absolute
against cwd, uncanonicalized (preserving the absolute-path invariant and symlink identity):

```rust
let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
let root = config
    .default_suite_path()
    .map(|rel| if rel.is_absolute() { rel } else { cwd.join(rel) })
    .unwrap_or(cwd);
```

`lsp.rs` already loads `ProjectConfig` (currently only for config_vars); it now also
derives the root from it. If no suite resolves, it falls back to cwd (the current
behavior) so the server always starts.

### Fix 5 — §2.3b overlay-URI (proef-lsp/src/documents.rs)

Key the overlay by **source name** instead of the raw `Uri`. Because `url_to_name`
*decodes* percent-encoding, the client's encoding choice stops mattering — both
`file:///a(b` and `file:///a%28b` map to the same name that the disk provider renders.

```rust
pub struct Documents {
    open: HashMap<String, Arc<str>>, // keyed by url_to_name(uri), not Uri
}

impl Documents {
    pub fn open(&mut self, url: Uri, text: String)  { self.open.insert(url_to_name(&url), Arc::from(text)); }
    pub fn change(&mut self, url: Uri, text: String) { self.open.insert(url_to_name(&url), Arc::from(text)); }
    pub fn close(&mut self, url: &Uri)               { self.open.remove(&url_to_name(url)); }
    pub fn get(&self, name: &str) -> Option<&str>    { self.open.get(name).map(|t| &**t) }
}
```

`OverlaySourceProvider::read(name)` then looks up `self.overlay.get(name)` directly —
no `name_to_url` round-trip in the read hot path. The existing `documents.rs` tests
switch to name-based `get`, and a new test opens a doc at a URI whose path contains a
sub-delim (`(`), edits it, and asserts the overlay bytes (not disk) are what analysis
reads. (The publish side already keys `published` by name and derives the URI only when
sending, so it is unaffected; a client matching diagnostics by normalized path is the
standard client behavior and out of proef's control.)

---

## Testing strategy

Every fix ships with the test that would have caught it. Flake rule (per
TESTING-STRATEGY): assert observable outcomes and normalized order, never wall-clock
interleaving.

- **§2.1 stdio lifecycle (new, proef-cli/tests/lsp_stdio.rs).** Spawn `proef lsp` as a
  subprocess (`assert_cmd`/`Command` with piped stdin/stdout) in an empty temp dir.
  Write LSP-framed (`Content-Length`) `initialize` → read the result → `initialized` →
  `shutdown` → read the ack → `exit`. Assert the process **exits** (status 0) within a
  watchdog timeout (a thread that kills the child and fails the test if it hangs). This
  is the only path that exercises the real `IoThreads`; without it §2.1 can regress
  invisibly.
- **§2.2 malformed params (new, proef-lsp/tests/lsp.rs).** Over `Connection::memory()`
  + `FakeDisk`, send a `textDocument/definition` request with hand-built params carrying
  a raw-space URI (`serde_json::json!` bypassing the typed builder). Assert the response
  is an error with code `-32602`. Then send a valid request (or `shutdown`/`exit`) and
  assert it is answered normally — proving the server survived.
- **§2.3b degradation (replaces analyze.rs `analyze_broken_pack_short_circuits…`).** That
  test pins the *bug*; replace it with a degradation test: a good pack (macro `greet`) +
  an unrelated broken pack + a feature using `greet`. Assert the broken pack carries its
  `proef::pack::yaml` diagnostic AND the `greet` binding still surfaces AND the good
  macro is in `macros` — proving one broken pack no longer zeroes the suite.
- **§2.3a root (proef-cli).** Unit-test the pure `default_suite_path`: `[run] suite` when
  set; `tests/` when the dir exists; `None` otherwise. (The full IO rooting is covered by
  the degradation + existing integration behavior.)
- **§2.3b overlay (new, documents.rs).** The sub-delim overlay test described above.
- **Regression:** the full existing LSP suite (tests/lsp.rs, server.rs, documents.rs)
  stays green, including on Windows (reuse `native_abs()`).

---

## Documentation plan

- **docs/CHANGELOG.md** — entries under `## [Unreleased]` (accumulates until release):
  the leak fix, the malformed-request hardening, broken-pack degradation, suite-scoped
  rooting, overlay name-keying, and the docs currency additions.
- **docs/EDITORS.md** — update the rooting description (line 14, 39-42) to "the configured
  `[run] suite` (else `tests/`) under the launch directory"; **remove** the overlay-URI
  limitation (113-116) now that it is fixed. Leave the go-to-def claim (100-102) and the
  config-restart limitation (95-99) untouched — both are still accurate.
- **README.md / docs/README.md** — add `proef-lsp` and the `lsp` subcommand to the crate
  list / command overview; add the 4 subcommands (`macros`/`diff`/`report`/`lsp`) where
  the command list omits them. (Read each file first; add only what is genuinely missing.)
- **docs/TECH-SPEC.md** — §10 CLI reference: add `lsp` (+ any of `macros`/`diff`/`report`
  missing). §14 Dependencies: add `lsp-server`/`lsp-types` to the pinned list and reconcile
  the "Datetime never enters our code" line (315) with the jiff-in-our-code rule. (§2 line
  51 already lists proef-lsp — leave it.)
- **docs/RELEASING.md** — add `proef-lsp` to the `[workspace.dependencies]` inter-crate pin
  list (line 62) and to the `cargo publish` order (proef-lsp before `proef`, since `proef`
  depends on it non-optionally). Read the section first; fix the concrete gap.
- **public-api** — regenerate `crates/proef-core/public-api.txt`; expected **unchanged**
  (the only core addition is `pub(crate)`). proef-lsp has no tracked public-api.

---

## Global constraints

- proef-core stays sans-IO (no clocks/env/IO/randomness in core); the degradation change
  adds no IO — it only stops discarding an already-built value.
- hurl pins `=8.0.1` untouched; no new dependencies anywhere.
- No task ids / plan numbers in code comments (changelog only).
- No AI-attribution trailers in commit messages.
- The existing LSP test suite stays green, including on Windows (`native_abs()` for paths).
- Public API unchanged: `load_collecting` is `pub(crate)`; regen confirms no delta.
- Ships as **v0.5.1** (patch — bug fixes + docs only; no behavior change to a successful run).
- Every gate, every task: `cargo fmt --all --check`; `cargo clippy --all-targets
  --all-features -- -D warnings`; `cargo nextest run --profile ci`; `cargo test --doc`;
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace`;
  `cargo run -p xtask -- docs-check`.

---

## Task breakdown (preview for the implementation plan)

1. **proef-core — degradation.** `pack::load_collecting` extraction + `load` wrapper;
   `analyze_suite` uses it (drop the early return); replace the short-circuit test with a
   degradation test; regen public-api (expect unchanged).
2. **proef-lsp — §2.1 leak + stdio lifecycle test.** `drop(connection)` before `join()`;
   new `proef-cli/tests/lsp_stdio.rs` subprocess test.
3. **proef-lsp — §2.2 malformed params + test.** `parse_params` helper + all three arms;
   new malformed-params test in `tests/lsp.rs`.
4. **proef-cli — §2.3a root-at-suite.** `ProjectConfig::default_suite_path`; `resolve_suite_path`
   delegates; `lsp.rs` roots at the suite; unit test for `default_suite_path`.
5. **proef-lsp — §2.3b overlay name-keying.** `Documents` keyed by name; `OverlaySourceProvider::read`
   direct lookup; update documents.rs tests + add the sub-delim overlay test.
6. **docs — currency sweep.** CHANGELOG, EDITORS.md, README, docs/README, TECH-SPEC §10+§14,
   RELEASING; docs-check green.

Order: 1 → 2 → 3 → 4 → 5 → 6. Tasks 2/3/5 all touch proef-lsp but different files/functions;
SDD runs one implementer at a time, so sequential edits do not conflict.

---

## Release note (out of this plan)

The version bump (`[workspace.package] version` 0.5.0 → 0.5.1 + the three inter-crate
`[workspace.dependencies]` pins) and the `vX.Y.Z` tag / crates.io publish are the separate
RELEASING runbook step, done once after this branch (and any coordinated PR #4 sequencing)
lands. The CHANGELOG stays under `[Unreleased]` here. Sequencing note: if v0.5.1 ships
before PR #4, PR #4 must carry its own EDITORS.md go-to-def doc update when it lands.
