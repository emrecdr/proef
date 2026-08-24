//! `render_html` — a self-contained HTML view of a run's event stream.
//!
//! A *derived* view (ADR-0008), never a second record: it replays the same
//! `events.jsonl` the console and `JUnit` reporters consume. Pure and
//! deterministic in `events` (sans-IO core), so it snapshot-locks like the
//! emitter. Events reaching here are already redacted at the sink
//! (`report::sink`), so no secret value can enter the page — the same
//! assumption `explain` makes.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::emit::slugify;
use crate::event::Event;
use crate::step::Status;

/// One step's row in the report.
struct StepRow {
    line: usize,
    text: String,
    status: Status,
    attempts: u32,
    duration_ms: u64,
    detail: Option<String>,
    /// The redacted curl of the failing request, straight from the record.
    reproduce_hint: Option<String>,
    /// `file.hurl#name` when the step ran a named fragment (ADR-0018).
    fragment: Option<String>,
}

/// Fold one `step_finished` into its scenario block; returns the attempts
/// folded so the caller's total stays in one place.
fn push_step_row(block: &mut ScenarioBlock, event: &Event) -> u64 {
    let Event::StepFinished {
        step,
        status,
        attempts,
        duration_ms,
        detail,
        fragment,
        reproduce_hint,
        ..
    } = event
    else {
        return 0;
    };
    block.steps.push(StepRow {
        line: step.line,
        text: step.text.to_string(),
        status: *status,
        attempts: *attempts,
        duration_ms: *duration_ms,
        detail: detail.clone(),
        reproduce_hint: reproduce_hint.clone(),
        fragment: fragment.clone(),
    });
    u64::from(*attempts)
}

/// One scenario's block: identity, aggregate status, and its steps in order.
#[derive(Default)]
struct ScenarioBlock {
    file: String,
    name: String,
    status: Option<Status>,
    /// Why `Skipped`, when the record says so.
    reason: Option<String>,
    /// The scenario's tags, straight from `scenario_finished`.
    tags: Vec<String>,
    /// `[run]` phase label, when the record says so — the tag table
    /// excludes phase scenarios exactly as every other total does.
    phase: Option<String>,
    steps: Vec<StepRow>,
    /// Run-relative start/end ms and worker index — injected observability
    /// (ADR-0015), present only when the record carries timing. When present
    /// they drive the cross-worker run timeline; absent, the report falls back
    /// to the per-scenario waterfalls alone.
    start_ms: Option<u64>,
    end_ms: Option<u64>,
    worker: Option<u64>,
}

/// Render `events` as a standalone HTML document. `artifacts_href` is the link
/// prefix for each scenario's `.hurl` artifact (e.g. `"artifacts"`, resolved
/// relative to wherever the caller writes the file); the artifact filename is
/// derived with the same slug the emitter uses, so the links match on disk.
pub fn render_html(events: &[Event], artifacts_href: &str) -> String {
    let mut run_id = String::new();
    let mut blocks: Vec<ScenarioBlock> = Vec::new();
    let mut index: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut total_steps = 0usize;
    let mut total_attempts = 0u64;
    let mut env: Option<String> = None;
    let mut metadata: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let mut run_finished: Option<(usize, usize, usize)> = None;

    for event in events {
        match event {
            Event::RunStarted {
                run_id: id,
                env: head_env,
                metadata: head_metadata,
                ..
            } => {
                run_id = id.to_string();
                env = head_env.as_ref().map(ToString::to_string);
                metadata = head_metadata.clone();
            }
            Event::StepFinished { scenario, step, .. } => {
                total_steps += 1;
                let at = block_index(&mut blocks, &mut index, &step.file, scenario);
                total_attempts += push_step_row(&mut blocks[at], event);
            }
            Event::ScenarioStarted {
                scenario,
                file,
                timestamp_ms,
                worker,
                ..
            } => {
                let at = block_index(&mut blocks, &mut index, file, scenario);
                blocks[at].start_ms = *timestamp_ms;
                blocks[at].worker = *worker;
            }
            Event::ScenarioFinished {
                scenario,
                file,
                status,
                timestamp_ms,
                worker,
                reason,
                tags,
                phase,
                ..
            } => {
                let at = block_index(&mut blocks, &mut index, file, scenario);
                blocks[at].status = Some(*status);
                blocks[at].reason = reason.as_ref().map(ToString::to_string);
                blocks[at].tags.clone_from(tags);
                blocks[at].phase = phase.as_ref().map(ToString::to_string);
                blocks[at].end_ms = *timestamp_ms;
                if blocks[at].worker.is_none() {
                    blocks[at].worker = *worker;
                }
            }
            Event::RunFinished {
                passed,
                failed,
                skipped,
                ..
            } => {
                run_finished = Some((*passed, *failed, *skipped));
            }
            _ => {}
        }
    }

    let (passed, failed, skipped) = suite_totals(run_finished, &blocks);
    // `warned` is informational only — never part of the aligned three
    // numbers above (no other surface breaks it out either) — so it stays a
    // plain count of every rendered block regardless of phase.
    let warned = blocks
        .iter()
        .filter(|block| block.status == Some(Status::Warned))
        .count();

    let mut html = String::with_capacity(2048 + blocks.len() * 256);
    let _ = writeln!(
        html,
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>proef report — {run}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n\
         <h1>proef run <code>{run}</code></h1>",
        run = esc(&run_id)
    );
    render_provenance_and_summary(
        &mut html,
        env.as_deref(),
        &metadata,
        (passed, failed, skipped, warned),
        (total_steps, total_attempts),
    );
    render_tag_table(&mut html, &blocks);
    render_timeline(&mut html, &blocks);
    for block in &blocks {
        render_block(&mut html, block, artifacts_href, failed);
    }
    html.push_str("</body>\n</html>\n");
    html
}

/// The headline `(passed, failed, skipped)` — mirrors every other surface
/// that reads `RunFinished` (console `summary:`, `explain`, `--output json`,
/// `JUnit`, TAP, the SLA gate, the exit code): main-suite scenarios only,
/// `[run] setup`/`teardown` excluded (ADR-0014). A truncated record has no
/// `RunFinished` to read, so fall back to counting every rendered block — the
/// only totals a dead run can offer.
fn suite_totals(
    run_finished: Option<(usize, usize, usize)>,
    blocks: &[ScenarioBlock],
) -> (usize, usize, usize) {
    run_finished.unwrap_or_else(|| {
        let (mut passed, mut failed, mut skipped) = (0usize, 0usize, 0usize);
        for block in blocks {
            match block.status {
                Some(Status::Passed) => passed += 1,
                Some(Status::Failed) => failed += 1,
                Some(Status::Skipped) => skipped += 1,
                Some(Status::Warned) | None => {}
            }
        }
        (passed, failed, skipped)
    })
}

/// Find or create the block for `(file, scenario)`, preserving first-seen order.
fn block_index(
    blocks: &mut Vec<ScenarioBlock>,
    index: &mut BTreeMap<(String, String), usize>,
    file: &str,
    scenario: &str,
) -> usize {
    *index
        .entry((file.to_string(), scenario.to_string()))
        .or_insert_with(|| {
            blocks.push(ScenarioBlock {
                file: file.to_string(),
                name: scenario.to_string(),
                ..ScenarioBlock::default()
            });
            blocks.len() - 1
        })
}

/// One `<details>` per scenario — failures open by default so the report leads
/// with what broke. `headline_failed` is the aligned failed count in the
/// summary bar above (`RunFinished`'s, suite-only per ADR-0014, or the
/// counted-block fallback on a truncated record): a block whose own status is
/// `Failed` while that count is `0` cannot be one of the failures the
/// headline counts — it is necessarily a `[run] setup`/`teardown` fault
/// excluded from it, so it is flagged here rather than left to read as the
/// page contradicting its own summary.
fn render_block(
    html: &mut String,
    block: &ScenarioBlock,
    artifacts_href: &str,
    headline_failed: usize,
) {
    let status = block.status.unwrap_or(Status::Skipped);
    let open = if status == Status::Failed {
        " open"
    } else {
        ""
    };
    let _ = write!(
        html,
        "<details class=\"scenario {cls}\"{open}>\n<summary>\
         <span class=\"pill {cls}\">{word}</span> \
         <span class=\"loc\">{file}</span> {name}",
        cls = status_class(status),
        word = status_word(status),
        file = esc(&block.file),
        name = esc(&block.name),
    );
    if let Some(reason) = &block.reason {
        let _ = write!(html, " <span class=\"loc\">— {}</span>", esc(reason));
    }
    if status == Status::Failed && headline_failed == 0 {
        html.push_str(
            " <span class=\"phase-note\">setup/teardown — excluded from totals above</span>",
        );
    }
    // Link the artifact only when the scenario actually ran hurl steps (else
    // no `.hurl` was emitted for it).
    if !block.steps.is_empty() {
        let stem = Path::new(&block.file).file_stem().map_or_else(
            || "feature".to_owned(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        let slug = format!("{}--{}", slugify(&stem), slugify(&block.name));
        let _ = write!(
            html,
            " <a class=\"artifact\" href=\"{href}/{slug}.hurl\">artifact</a>",
            href = esc(artifacts_href),
        );
    }
    html.push_str("</summary>\n<ol class=\"steps\">\n");
    // Per-scenario timing waterfall: each step's bar is offset by the steps
    // before it and as wide as its own duration, both as a fraction of the
    // scenario total. Purely derived from `duration_ms` (no timestamps), so it
    // shows the *sequential* cascade within one scenario — not cross-worker
    // occupancy, which would need an injected clock the sans-IO core never reads.
    let total_ms: u64 = block.steps.iter().map(|step| step.duration_ms).sum();
    let mut elapsed_ms: u64 = 0;
    for step in &block.steps {
        let _ = write!(
            html,
            "<li class=\"{cls}\"><span class=\"glyph\">{glyph}</span> {text}\
             <span class=\"meta\">:{line} · {attempts}× · {ms}ms</span>",
            cls = status_class(step.status),
            glyph = status_glyph(step.status),
            text = esc(&step.text),
            line = step.line,
            attempts = step.attempts,
            ms = step.duration_ms,
        );
        if total_ms > 0 {
            let _ = write!(
                html,
                "<span class=\"track\"><span class=\"bar {cls}\" \
                 style=\"margin-left:{offset}%;width:{width}%\"></span></span>",
                cls = status_class(step.status),
                offset = pct(elapsed_ms, total_ms),
                width = pct(step.duration_ms, total_ms),
            );
        }
        elapsed_ms += step.duration_ms;
        if let Some(detail) = &step.detail {
            let _ = write!(html, "<pre class=\"detail\">{}</pre>", esc(detail));
        }
        if let Some(hint) = &step.reproduce_hint {
            let _ = write!(html, "<pre class=\"detail\">reproduce: {}</pre>", esc(hint));
        }
        // Every `ref:` step, not only failing ones: this is a per-step listing
        // rather than a failure list, so a green report answers "which file did
        // this run" too. It sits last so a failure still reads reason-first
        // (ADR-0018).
        if let Some(fragment) = &step.fragment {
            let _ = write!(html, "<p class=\"via\">via {}</p>", esc(fragment));
        }
        html.push_str("</li>\n");
    }
    html.push_str("</ol>\n</details>\n");
}

/// The cross-worker run timeline (ADR-0015): a lane per worker, each scenario a
/// bar from its start to its finish, positioned on a shared run-relative axis so
/// concurrency is visible at a glance. Rendered only when the record carries
/// injected timing (`start`/`end` timestamps); absent, the report shows just the
/// Per-tag verdicts — the rollup a fifty-scenario suite is read by (RF's
/// "statistics by tag", the shape a requirement-tagged suite turns into a
/// traceability matrix). Suite-only, exactly like every other total
/// (ADR-0014: phase scenarios are excluded); Warned counts with passed,
/// mirroring `RunSummary::passed`; time is the sum of the block's step
/// durations, so it works on records without injected timing. Rendered only
/// when a suite scenario carries a tag — a tagless suite gets no empty
/// The header strip under the `<h1>`: explicit provenance (env, metadata —
/// ADR-0020) when present, then the totals bar.
fn render_provenance_and_summary(
    html: &mut String,
    env: Option<&str>,
    metadata: &std::collections::BTreeMap<String, String>,
    totals: (usize, usize, usize, usize),
    steps: (usize, u64),
) {
    let (passed, failed, skipped, warned) = totals;
    let (total_steps, total_attempts) = steps;
    if env.is_some() || !metadata.is_empty() {
        html.push_str("<p class=\"summary\">");
        if let Some(env) = env {
            let _ = write!(html, "<span class=\"steps\">env: {}</span> ", esc(env));
        }
        for (key, value) in metadata {
            let _ = write!(
                html,
                "<span class=\"steps\">{}: {}</span> ",
                esc(key),
                esc(value)
            );
        }
        html.push_str("</p>\n");
    }
    html.push_str("<p class=\"summary\">");
    tally(html, "pass", passed, "passed");
    tally(html, "fail", failed, "failed");
    tally(html, "skip", skipped, "skipped");
    if warned > 0 {
        tally(html, "warn", warned, "warned");
    }
    let _ = writeln!(
        html,
        "<span class=\"steps\">{total_steps} steps · {total_attempts} attempts</span></p>"
    );
}

/// section, and pre-field records are unchanged.
fn render_tag_table(html: &mut String, blocks: &[ScenarioBlock]) {
    use std::collections::BTreeMap;
    let mut rows: BTreeMap<&str, (usize, usize, usize, u64)> = BTreeMap::new();
    for block in blocks {
        if block.phase.is_some() {
            continue;
        }
        let status = block.status.unwrap_or(Status::Skipped);
        let time: u64 = block.steps.iter().map(|s| s.duration_ms).sum();
        for tag in &block.tags {
            let row = rows.entry(tag.as_str()).or_default();
            match status {
                Status::Passed | Status::Warned => row.0 += 1,
                Status::Failed => row.1 += 1,
                Status::Skipped => row.2 += 1,
            }
            row.3 += time;
        }
    }
    if rows.is_empty() {
        return;
    }
    html.push_str(
        "<table class=\"tags\">\n<thead><tr><th>tag</th><th>passed</th>\
         <th>failed</th><th>skipped</th><th>time</th></tr></thead>\n<tbody>\n",
    );
    for (tag, (passed, failed, skipped, ms)) in rows {
        let _ = writeln!(
            html,
            "<tr><td>@{}</td><td>{passed}</td><td>{failed}</td><td>{skipped}</td>\
             <td>{ms}ms</td></tr>",
            esc(tag)
        );
    }
    html.push_str("</tbody>\n</table>\n");
}

/// per-scenario waterfalls, so old records degrade cleanly.
fn render_timeline(html: &mut String, blocks: &[ScenarioBlock]) {
    let timed: Vec<&ScenarioBlock> = blocks
        .iter()
        .filter(|block| block.start_ms.is_some() && block.end_ms.is_some())
        .collect();
    let Some(max_end) = timed.iter().filter_map(|block| block.end_ms).max() else {
        return; // no timed scenarios — old record, waterfalls only
    };
    if max_end == 0 {
        return; // a zero-length run has nothing to place on the axis
    }
    let mut workers: Vec<u64> = timed
        .iter()
        .map(|block| block.worker.unwrap_or(0))
        .collect();
    workers.sort_unstable();
    workers.dedup();

    let _ = writeln!(
        html,
        "<h2 class=\"timeline-h\">Timeline <span class=\"count\">{max_end}ms</span></h2>\n\
         <div class=\"timeline\">"
    );
    for worker in &workers {
        let _ = write!(
            html,
            "<div class=\"lane\"><span class=\"lane-label\">worker {worker}</span>\
             <div class=\"lane-track\">"
        );
        for block in timed
            .iter()
            .filter(|block| block.worker.unwrap_or(0) == *worker)
        {
            let start = block.start_ms.unwrap_or(0);
            let end = block.end_ms.unwrap_or(start).max(start);
            let _ = write!(
                html,
                "<span class=\"tbar {cls}\" style=\"left:{left}%;width:{width}%\" \
                 title=\"{title} ({dur}ms)\"></span>",
                cls = status_class(block.status.unwrap_or(Status::Skipped)),
                left = pct(start, max_end),
                width = pct(end - start, max_end),
                title = esc(&block.name),
                dur = end - start,
            );
        }
        html.push_str("</div></div>\n");
    }
    html.push_str("</div>\n");
}

/// `n / total` as a percentage string with one decimal place, using integer
/// math only (no lossy float cast). `total` must be non-zero (callers guard).
fn pct(n: u64, total: u64) -> String {
    let permille = u128::from(n) * 1000 / u128::from(total); // 0..=1000
    format!("{}.{}", permille / 10, permille % 10)
}

/// Write one `<span>` count into the summary bar, omitting nothing (callers gate
/// on zero where a bucket should hide).
fn tally(html: &mut String, class: &str, count: usize, word: &str) {
    let _ = write!(html, "<span class=\"count {class}\">{count} {word}</span> ");
}

fn status_class(status: Status) -> &'static str {
    match status {
        Status::Passed => "pass",
        Status::Failed => "fail",
        Status::Skipped => "skip",
        Status::Warned => "warn",
    }
}

fn status_word(status: Status) -> &'static str {
    match status {
        Status::Passed => "passed",
        Status::Failed => "failed",
        Status::Skipped => "skipped",
        Status::Warned => "warned",
    }
}

fn status_glyph(status: Status) -> &'static str {
    match status {
        Status::Passed => "✓",
        Status::Failed => "✗",
        Status::Skipped => "·",
        Status::Warned => "⚠",
    }
}

/// HTML-escape text destined for element content or a double-quoted attribute.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Inlined stylesheet — the report is a single self-contained file (no external
/// requests), light/dark aware for a local artifact.
const STYLE: &str = "\
:root{--bg:#fff;--fg:#1a1a1a;--muted:#666;--line:#e2e2e2;--pass:#1a7f37;--fail:#cf222e;--skip:#8a8a8a;--warn:#9a6700;--card:#f6f8fa}\
@media(prefers-color-scheme:dark){:root{--bg:#0d1117;--fg:#e6edf3;--muted:#9aa4af;--line:#30363d;--pass:#3fb950;--fail:#f85149;--skip:#8a8a8a;--warn:#d29922;--card:#161b22}}\
*{box-sizing:border-box}body{margin:0;padding:2rem;max-width:60rem;margin:0 auto;background:var(--bg);color:var(--fg);font:15px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif}\
h1{font-size:1.4rem;font-weight:600}code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace}\
.incomplete-banner{color:var(--warn);font-weight:600;margin:0 0 1rem}\
.summary{display:flex;flex-wrap:wrap;gap:.5rem;align-items:center;margin:0 0 1.5rem}\
.count{font-weight:600;padding:.15rem .6rem;border-radius:999px;background:var(--card)}\
.count.pass{color:var(--pass)}.count.fail{color:var(--fail)}.count.skip{color:var(--skip)}.count.warn{color:var(--warn)}\
.summary .steps{color:var(--muted);margin-left:auto}\
.scenario{border:1px solid var(--line);border-radius:8px;margin:.5rem 0;background:var(--card)}\
.scenario summary{cursor:pointer;padding:.6rem .8rem;list-style:none;display:flex;align-items:center;gap:.5rem;flex-wrap:wrap}\
.scenario summary::-webkit-details-marker{display:none}\
.pill{font-size:.72rem;font-weight:700;text-transform:uppercase;letter-spacing:.03em;padding:.1rem .5rem;border-radius:4px;color:#fff}\
.pill.pass{background:var(--pass)}.pill.fail{background:var(--fail)}.pill.skip{background:var(--skip)}.pill.warn{background:var(--warn)}\
.loc{color:var(--muted);font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.85rem}\
.artifact{margin-left:auto;font-size:.85rem;color:var(--muted)}\
.phase-note{color:var(--muted);font-size:.78rem;font-style:italic}\
     .tags{border-collapse:collapse;margin:.6rem 0 1rem;font-size:.85rem}\
     .tags th,.tags td{border:1px solid var(--line);padding:.25rem .6rem;text-align:left}\
     .tags th{color:var(--muted);font-weight:600}\
.steps{margin:0;padding:.2rem .8rem .8rem 2rem;border-top:1px solid var(--line)}\
.steps li{margin:.3rem 0}.steps .glyph{font-weight:700}\
li.pass .glyph{color:var(--pass)}li.fail .glyph{color:var(--fail)}li.skip .glyph{color:var(--skip)}li.warn .glyph{color:var(--warn)}\
.meta{color:var(--muted);font-size:.8rem;margin-left:.4rem;font-family:ui-monospace,SFMono-Regular,Menlo,monospace}\
.track{display:block;height:4px;margin:.25rem 0 0;background:var(--line);border-radius:2px;overflow:hidden}\
.bar{display:block;height:100%;min-width:1px;border-radius:2px}\
.bar.pass{background:var(--pass)}.bar.fail{background:var(--fail)}.bar.skip{background:var(--skip)}.bar.warn{background:var(--warn)}\
.timeline-h{font-size:1.05rem;font-weight:600;margin:1.5rem 0 .5rem}\
.timeline{margin:0 0 1.5rem}\
.lane{display:flex;align-items:center;gap:.5rem;margin:.25rem 0}\
.lane-label{color:var(--muted);font-size:.75rem;font-family:ui-monospace,SFMono-Regular,Menlo,monospace;min-width:5rem;text-align:right}\
.lane-track{position:relative;flex:1;height:1rem;background:var(--card);border:1px solid var(--line);border-radius:3px}\
.tbar{position:absolute;top:1px;height:calc(100% - 2px);min-width:2px;border-radius:2px;opacity:.9}\
.tbar.pass{background:var(--pass)}.tbar.fail{background:var(--fail)}.tbar.skip{background:var(--skip)}.tbar.warn{background:var(--warn)}\
.detail{background:var(--bg);border:1px solid var(--line);border-radius:6px;padding:.5rem .7rem;margin:.4rem 0 0;white-space:pre-wrap;font-size:.82rem;overflow-x:auto}\
.via{color:var(--muted);font-size:.78rem;font-family:ui-monospace,SFMono-Regular,Menlo,monospace;margin:.3rem 0 0}\
";
