# Upstream PR briefing — `run_entries`: accept a caller-provided HTTP client

**Target:** `Orange-OpenSource/hurl` · **Patch:**
[`0001-run-entries-reusable-client.patch`](0001-run-entries-reusable-client.patch)
(verified to apply and compile against 8.0.1 **and** master @ `03fcb84`).

> **Process (from hurl's CONTRIBUTING.md — read before acting):**
> 1. An **issue/discussion must precede any PR**.
> 2. All commits must be **signed and GitHub-Verified** (configure GPG/SSH
>    signing first; the prepared branch's commit must be amended + signed).
> 3. Their **AI Tool Use Policy** requires issues, PR descriptions, and all
>    other communications to be **written by a human** — treat everything
>    below as a technical briefing to write your own text from, not as text
>    to paste.
>
> A ready branch (`run-entries-reusable-client`) is pushed to the
> `emrecdr/hurl` fork; amend-and-sign it when opening the PR.

---

## What

`runner::run_entries` currently constructs its `http::Client` internally
(`runner/hurl_file.rs`), so every call starts with a fresh libcurl handle: the
connection cache and the cookie jar do not survive across calls. This PR lifts
the client into a parameter:

```rust
pub fn run_entries(
    entries: &[Entry],
    content: &str,
    filename: Option<&Input>,
    http_client: &mut Client,   // new
    runner_options: &RunnerOptions,
    ...
```

Both internal callers (`run` in `runner/hurl_file.rs`, the parallel worker in
`parallel/worker.rs`) construct the client themselves — their behavior is
unchanged.

## Why

Embedders that drive hurl as a library sometimes need to execute one logical
file as *several* `run_entries` calls (for example: interleaving entries with
non-HTTP steps, or isolating optional entries whose failure should not abort
the rest). Today each segment pays reconnection costs and, worse, loses the
cookie jar — the only workaround is round-tripping
`CookieStore::to_netscape()` through a temp file into
`RunnerOptionsBuilder::cookie_input_file`, per segment.

With a caller-owned client, connection reuse and cookies simply persist across
segments, matching what a single-call run already does.

## Compatibility

- `runner::run` (the documented entry point) is unchanged.
- `run_entries` is `#[doc(hidden)]`; its two in-tree callers are updated in
  this PR. For out-of-tree embedders this is a mechanical one-line change at
  each call site (`Client::new()` + pass `&mut client`).
- No behavior change for single-call usage: same client lifetime as before.

## Testing

- Patch compiles against 8.0.1 (`cargo build --lib`, verified).
- Suggested integration check: run one file as two `run_entries` calls over a
  cookie-setting endpoint and assert the second call sends the cookie without
  `cookie_input_file` (we run exactly this scenario downstream in proef's
  fixture suite and can contribute it as a test).
