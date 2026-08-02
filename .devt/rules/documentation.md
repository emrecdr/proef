# Documentation — proef

> Grounded in `CLAUDE.md`, `docs/CONTRIBUTING.md`, and `docs/README.md` (the corpus index).
> Those sources WIN on any conflict. Read by the `docs-writer` agent.

proef has two documentation surfaces: **rustdoc** on the code, and the **`docs/` corpus**
(the authoritative spec). `CLAUDE.md` is the working summary; `docs/TECH-SPEC.md` and the
ADRs win on any conflict.

## The `docs/` corpus — keep it in sync

- `docs/README.md` — corpus index + ADR decision log. Entry point.
- `docs/adr/` — the ADR series (ADR-0001 onward), decisions with alternatives +
  consequences. **A new architectural decision (or a divergence from an existing ADR)
  requires a new `docs/adr/ADR-00NN-*.md` — number it one past the highest existing file in
  `docs/adr/`, same format, in the same PR.** Diverging without a superseding ADR is a bug.
- `docs/TECH-SPEC.md` — normative types, pipeline, pack schema, and the verified hurl seam
  facts with file:line citations (§5). Do not re-derive these from priors; cite them.
- `docs/IMPLEMENTATION-PLAN.md` — milestones + the global definition of done.
- `docs/PRD.md` — user stories US-1..US-12 that acceptance criteria cite.
- `docs/DIAGNOSTICS.md` — every diagnostic code. **A new diagnostic code needs a row here**
  plus (where reachable) a seeded case under `tests/errors/<area>__<name>/`.
- `docs/EVENTS.md`, `docs/AUTHORING.md`, `docs/GETTING-STARTED.md`, `docs/RELEASING.md`,
  `docs/TROUBLESHOOTING.md`, `docs/CONFIG.md`, `docs/runbooks/` — keep accurate to behavior.
- **Keep the `CLAUDE.md` Status list current** as milestones land.

`cargo run -p xtask -- docs-check` verifies indexes match reality — run it after doc edits.

## Rustdoc conventions

- Public items get a doc comment (definition of done: "public items documented").
  `#![warn(missing_docs)]` on library crates.
- Start with a one-sentence summary, blank line, then detail. Describe behavior and
  contracts, not implementation.
- `# Errors` on `Result`-returning public fns; `# Panics` on fns that can panic on input;
  `# Safety` on any `unsafe fn` (proef is overwhelmingly safe code — `unsafe` is rare and
  reviewed).
- Every public item gets at least one example; examples run under `cargo test --doc`
  (nextest does not run doctests — run it separately).
- Intra-doc links (`[`OtherType`]`) over hand-written URLs — they survive renames and are
  validated by the `RUSTDOCFLAGS="-D warnings" cargo doc` gate.

## Documentation gates

```bash
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --workspace  # broken links + missing docs are errors
cargo test --doc                                                           # doc examples are real tests
cargo run -p xtask -- docs-check                                           # corpus indexes ↔ reality
cargo run -p xtask -- public-api                                           # proef-core surface (nightly rustdoc)
```

## proef-core public API is snapshot-locked

`crates/proef-core/public-api.txt` pins the surface. An intended change regenerates it:
`PROEF_PUBLIC_API_UPDATE=1 cargo run -p xtask -- public-api`. An unintended diff here means
you widened the API by accident — review before regenerating.

## What NOT to document in code comments

Comments state a constraint the code cannot show (a verified seam fact, an invariant to
preserve). Do not narrate what the next line does, restate the signature, or add
attribution/history — that belongs in git, not the source.
