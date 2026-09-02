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
    /// The pack step's authored `name:` — what tells two engine steps of one
    /// feature sentence apart.
    label: Option<String>,
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
        label,
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
        label: label.clone(),
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

/// Everything the event stream says about a run, folded once.
///
/// Named because it is a distinct job from rendering: the page is built from
/// this value, and nothing downstream re-reads the events.
struct Recorded {
    run_id: String,
    blocks: Vec<ScenarioBlock>,
    total_steps: usize,
    total_attempts: u64,
    env: Option<String>,
    metadata: std::collections::BTreeMap<String, String>,
    /// The last tail `RunFinished`'s totals, when the stream reached one.
    run_finished: Option<(usize, usize, usize)>,
    /// How many tail events the stream carried. More than one means a
    /// pre-0.6.0 record, whose totals counted each phase rather than the suite.
    run_finished_seen: usize,
}

fn fold_events(events: &[Event]) -> Recorded {
    let mut run_id = String::new();
    let mut blocks: Vec<ScenarioBlock> = Vec::new();
    let mut index: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut total_steps = 0usize;
    let mut total_attempts = 0u64;
    let mut env: Option<String> = None;
    let mut metadata: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let mut run_finished: Option<(usize, usize, usize)> = None;
    let mut run_finished_seen = 0usize;

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
                run_finished_seen += 1;
                run_finished = Some((*passed, *failed, *skipped));
            }
            _ => {}
        }
    }

    Recorded {
        run_id,
        blocks,
        total_steps,
        total_attempts,
        env,
        metadata,
        run_finished,
        run_finished_seen,
    }
}

/// Render `events` as a standalone HTML document. `artifacts_href` is the link
/// prefix for each scenario's `.hurl` artifact (e.g. `"artifacts"`, resolved
/// relative to wherever the caller writes the file); the artifact filename is
/// derived with the same slug the emitter uses, so the links match on disk.
pub fn render_html(
    events: &[Event],
    artifacts_href: &str,
    tag_links: &std::collections::BTreeMap<String, String>,
) -> String {
    let recorded = fold_events(events);
    let (passed, failed, skipped) = headline(&recorded);
    let Recorded {
        run_id,
        blocks,
        total_steps,
        total_attempts,
        env,
        metadata,
        ..
    } = &recorded;
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
        run = esc(run_id)
    );
    render_provenance_and_summary(
        &mut html,
        env.as_deref(),
        metadata,
        (passed, failed, skipped, warned),
        (*total_steps, *total_attempts),
    );
    render_failure_jump(&mut html, blocks);
    render_filter_bar(&mut html);
    render_tag_table(&mut html, blocks, tag_links);
    render_timeline(&mut html, blocks);
    render_slowest(&mut html, blocks);
    if !blocks.is_empty() {
        html.push_str("<h2 class=\"section-h\" id=\"scenarios\">Scenarios</h2>\n");
    }
    for block in blocks {
        render_block(&mut html, block, artifacts_href, failed);
    }
    html.push_str(FILTER_SCRIPT);
    html.push_str("</body>\n</html>\n");
    html
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

/// The status filter, appended before `</body>`. Vanilla and tiny: it only
/// toggles a class the stylesheet hides, over the status classes every block
/// already carries — no state, no framework, still one self-contained file.
const FILTER_SCRIPT: &str = r"<script>
for (const b of document.querySelectorAll('.filter button')) {
  b.addEventListener('click', () => {
    for (const o of document.querySelectorAll('.filter button')) o.classList.remove('on');
    b.classList.add('on');
    const f = b.dataset.f;
    for (const d of document.querySelectorAll('details.scenario'))
      d.classList.toggle('gone', f !== 'all' && !d.classList.contains(f));
  });
}
</script>
";

/// One slug per scenario block, shared by its `id=` anchor and its artifact
/// link — the same `stem--name` spelling the emitter uses for the `.hurl`
/// file, so the two can never disagree.
fn block_slug(block: &ScenarioBlock) -> String {
    let stem = Path::new(&block.file).file_stem().map_or_else(
        || "feature".to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    format!("{}--{}", slugify(&stem), slugify(&block.name))
}

/// The status filter's buttons. Progressive enhancement over classes the
/// blocks already carry: with scripting unavailable they do nothing and the
/// page stays the complete, ordered document it always was.
fn render_filter_bar(html: &mut String) {
    html.push_str(
        "<div class=\"filter\">show: <button class=\"on\" data-f=\"all\">all</button>\
         <button data-f=\"fail\">failed</button>\
         <button data-f=\"skip\">skipped</button>\
         <button data-f=\"warn\">warned</button></div>\n",
    );
}

/// The triage rail: every failing scenario, linked to its block. Blocks keep
/// first-seen order (completion order carries information), so the rail is
/// how a reader skips the green between failures — and how a failure gets a
/// shareable `#s-…` URL handed to a colleague.
fn render_failure_jump(html: &mut String, blocks: &[ScenarioBlock]) {
    let failing: Vec<&ScenarioBlock> = blocks
        .iter()
        .filter(|block| block.status == Some(Status::Failed))
        .collect();
    if failing.is_empty() {
        return;
    }
    html.push_str("<nav class=\"jump\">failed: ");
    for (i, block) in failing.iter().enumerate() {
        if i > 0 {
            html.push_str(" · ");
        }
        let _ = write!(
            html,
            "<a href=\"#s-{}\">{}</a>",
            block_slug(block),
            esc(&block.name)
        );
    }
    html.push_str("</nav>\n");
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
    // The id is what makes a failure *shareable*: triage is a handoff, and
    // a page whose blocks cannot be linked ends every handoff with "scroll
    // until you find it". `slugify` output only — no escaping context.
    let _ = write!(
        html,
        "<details class=\"scenario {cls}\" id=\"s-{slug}\"{open}>\n<summary>\
         <span class=\"pill {cls}\">{word}</span> \
         <span class=\"loc\">{file}</span> {name}",
        cls = status_class(status),
        slug = block_slug(block),
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
        let _ = write!(
            html,
            " <a class=\"artifact\" href=\"{href}/{slug}.hurl\">artifact</a>",
            href = esc(artifacts_href),
            slug = block_slug(block),
        );
    }
    html.push_str("</summary>\n<ol class=\"steps\">\n");
    // Per-scenario timing waterfall: each step's bar is offset by the steps
    // before it and as wide as its own duration, both as a fraction of the
    // scenario total. Purely derived from `duration_ms` (no timestamps), so it
    // shows the *sequential* cascade within one scenario — not cross-worker
    // occupancy, which would need an injected clock the sans-IO core never reads.
    let total_ms: u64 = block
        .steps
        .iter()
        .fold(0u64, |acc, step| acc.saturating_add(step.duration_ms));
    let mut elapsed_ms: u64 = 0;
    for step in &block.steps {
        let _ = write!(
            html,
            "<li class=\"{cls}\"><span class=\"glyph\">{glyph}</span> {text}{label}\
             <span class=\"meta\">:{line} · {attempts}× · {ms}ms</span>",
            cls = status_class(step.status),
            glyph = status_glyph(step.status),
            text = esc(&step.text),
            label = step
                .label
                .as_deref()
                .map(|name| format!("<span class=\"steplabel\"> › {}</span>", esc(name)))
                .unwrap_or_default(),
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
        elapsed_ms = elapsed_ms.saturating_add(step.duration_ms);
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

/// The headline `(passed, failed, skipped)`, through the one rule every surface
/// shares ([`crate::report::suite_totals`]): the tail `RunFinished` when it can
/// be trusted, else suite-only counting with `Warned` riding along with
/// `Passed`. This page used to carry its own fallback, which disagreed with
/// `explain` on all three points for the same bytes.
fn headline(recorded: &Recorded) -> (usize, usize, usize) {
    crate::report::suite_totals(
        recorded.run_finished,
        // More than one tail event means a pre-0.6.0 record: it emitted one
        // head/tail pair per phase and its totals counted every phase rather
        // than the suite, so keeping only the last one reported that phase's
        // numbers as the run's verdict.
        recorded.run_finished_seen > 1,
        recorded
            .blocks
            .iter()
            .map(|block| (block.phase.is_none(), block.status)),
    )
}

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
/// Per-tag verdicts — the rollup a fifty-scenario suite is read by (RF's
/// "statistics by tag", the shape a requirement-tagged suite turns into a
/// traceability matrix). Suite-only, exactly like every other total
/// (ADR-0014: phase scenarios are excluded); Warned counts with passed,
/// mirroring `RunSummary::passed`; time is the sum of the block's step
/// durations, so it works on records without injected timing. Rendered only
/// when a suite scenario carries a tag — a tagless suite gets no empty table.
fn render_tag_table(
    html: &mut String,
    blocks: &[ScenarioBlock],
    tag_links: &std::collections::BTreeMap<String, String>,
) {
    use std::collections::BTreeMap;
    let mut rows: BTreeMap<&str, (usize, usize, usize, u64)> = BTreeMap::new();
    for block in blocks {
        if block.phase.is_some() {
            continue;
        }
        let status = block.status.unwrap_or(Status::Skipped);
        let time: u64 = block
            .steps
            .iter()
            .fold(0u64, |acc, s| acc.saturating_add(s.duration_ms));
        for tag in &block.tags {
            let row = rows.entry(tag.as_str()).or_default();
            match status {
                Status::Passed | Status::Warned => row.0 += 1,
                Status::Failed => row.1 += 1,
                Status::Skipped => row.2 += 1,
            }
            row.3 = row.3.saturating_add(time);
        }
    }
    if rows.is_empty() {
        return;
    }
    html.push_str(
        "<h2 class=\"section-h\" id=\"by-tag\">By tag</h2>\n\
         <table class=\"tags\">\n<thead><tr><th>tag</th><th>passed</th>\
         <th>failed</th><th>skipped</th><th>time</th></tr></thead>\n<tbody>\n",
    );
    for (tag, (passed, failed, skipped, ms)) in rows {
        // A `[tag-links]` glob turns the tag cell into a tracker link —
        // `@JIRA-123` clicks through to the issue (RF's --tagstatlink,
        // reduced to one mechanism: the existing tag glob + `{tag}`).
        // Non-http(s) templates render as plain text: `esc` keeps the href
        // *well-formed*, but a `javascript:` template (or a bare `{tag}`
        // template letting the tag become the whole scheme) would still be a
        // live link — the GitHub-summary sink applies the same rule.
        let cell = tag_links
            .iter()
            .find(|(pattern, _)| crate::tags::atom_matches(pattern, tag))
            .filter(|(_, template)| {
                template.starts_with("https://") || template.starts_with("http://")
            })
            .map_or_else(
                || format!("@{}", esc(tag)),
                |(_, template)| {
                    let url = template.replace("{tag}", tag);
                    format!("<a href=\"{}\">@{}</a>", esc(&url), esc(tag))
                },
            );
        let _ = writeln!(
            html,
            "<tr><td>{cell}</td><td>{passed}</td><td>{failed}</td><td>{skipped}</td>\
             <td>{ms}ms</td></tr>"
        );
    }
    html.push_str("</tbody>\n</table>\n");
}

/// per-scenario waterfalls, so old records degrade cleanly.
/// The cross-worker run timeline (ADR-0015): a lane per worker, each scenario a
/// bar from its start to its finish, positioned on a shared run-relative axis so
/// concurrency is visible at a glance. Rendered only when the record carries
/// injected timing (`start`/`end` timestamps); absent, the report shows just the
/// scenario listing.
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
        "<h2 class=\"section-h\">Timeline <span class=\"count\">{max_end}ms</span></h2>\n\
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

/// How many scenarios the "slowest" section lists at most.
///
/// Small on purpose. The section answers "what should I attack first", and a
/// ranking long enough to need scrolling has stopped answering it.
const SLOWEST_SHOWN: usize = 8;

/// The slowest scenarios, ranked, with the share of run time they account for.
///
/// After "what failed", this is the question a test report is most often asked,
/// and until now the page could not answer it: the timeline showed *that*
/// workers were busy, never *which* scenarios to attack. Every number needed was
/// already in the fold.
///
/// Cost is the **sum of a scenario's step durations**, the same definition
/// `timings.json` uses for shard weights — one notion of "what a scenario
/// costs" across the whole tool. Deliberately not the scenario's wall-clock
/// span, which includes time spent waiting for a worker: that is a property of
/// how the run was scheduled, not of the scenario, and it is not something the
/// reader can go and fix.
///
/// The share is the actionable half. "These three are 71% of the suite" is a
/// decision; a list of durations is homework.
fn render_slowest(html: &mut String, blocks: &[ScenarioBlock]) {
    let mut ranked: Vec<(u64, &ScenarioBlock)> = blocks
        .iter()
        .map(|block| {
            let ms = block.steps.iter().map(|step| step.duration_ms).sum::<u64>();
            (ms, block)
        })
        .filter(|(ms, _)| *ms > 0)
        .collect();
    // Fewer than two timed scenarios cannot be ranked against each other, and a
    // "slowest" list of one is just that scenario's row repeated.
    if ranked.len() < 2 {
        return;
    }
    // Slowest first; ties by identity so the order is a function of the record
    // rather than of fold order.
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| (&a.1.file, &a.1.name).cmp(&(&b.1.file, &b.1.name)))
    });

    let total: u64 = ranked.iter().map(|(ms, _)| *ms).sum();
    let shown = ranked.len().min(SLOWEST_SHOWN);
    let covered: u64 = ranked.iter().take(shown).map(|(ms, _)| *ms).sum();
    let slowest = ranked.first().map_or(1, |(ms, _)| *ms).max(1);

    let _ = write!(
        html,
        "<h2 class=\"section-h\" id=\"slowest\">Slowest <span class=\"count\">\
         {shown} of {} · {}% of run time</span></h2>\n<ol class=\"slow\">\n",
        ranked.len(),
        pct(covered, total)
    );
    for (ms, block) in ranked.iter().take(shown) {
        let width = pct(*ms, slowest);
        let _ = writeln!(
            html,
            "<li><a href=\"#s-{slug}\">{name}</a>\
             <span class=\"slow-bar\"><span class=\"bar {cls}\" style=\"width:{width}%\"></span></span>\
             <span class=\"slow-ms\">{ms} ms</span></li>",
            slug = block_slug(block),
            name = esc(&block.name),
            cls = status_class(block.status.unwrap_or(Status::Passed)),
        );
    }
    html.push_str("</ol>\n");
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
:root{--bg:#fff;--fg:#1a1a1a;--muted:#666;--line:#e2e2e2;--pass:#1a7f37;--fail:#cf222e;--skip:#59636e;--warn:#9a6700;--card:#f6f8fa;--pill-fg:#fff}\
@media(prefers-color-scheme:dark){:root{--bg:#0d1117;--fg:#e6edf3;--muted:#9aa4af;--line:#30363d;--pass:#3fb950;--fail:#f85149;--skip:#8a8a8a;--warn:#d29922;--card:#161b22;--pill-fg:#0d1117}}\
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
.jump{margin:.4rem 0;font-size:.85rem}\
.filter{margin:.4rem 0 .8rem;font-size:.85rem}\
.filter button{border:1px solid var(--line,#ccc);background:transparent;color:inherit;border-radius:4px;padding:.1rem .6rem;margin-right:.3rem;cursor:pointer;font:inherit}\
.filter button.on{font-weight:700;border-color:currentColor}\
.scenario.gone{display:none}\
.pill{font-size:.72rem;font-weight:700;text-transform:uppercase;letter-spacing:.03em;padding:.1rem .5rem;border-radius:4px;color:var(--pill-fg)}\
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
.steplabel{color:var(--muted)}\
.track{display:block;height:4px;margin:.25rem 0 0;background:var(--line);border-radius:2px;overflow:hidden}\
.bar{display:block;height:100%;min-width:1px;border-radius:2px}\
.bar.pass{background:var(--pass)}.bar.fail{background:var(--fail)}.bar.skip{background:var(--skip)}.bar.warn{background:var(--warn)}\
.section-h{font-size:1.05rem;font-weight:600;margin:1.5rem 0 .5rem}\
.timeline{margin:0 0 1.5rem}\
.lane{display:flex;align-items:center;gap:.5rem;margin:.25rem 0}\
.lane-label{color:var(--muted);font-size:.75rem;font-family:ui-monospace,SFMono-Regular,Menlo,monospace;min-width:5rem;text-align:right}\
.lane-track{position:relative;flex:1;height:1rem;background:var(--card);border:1px solid var(--line);border-radius:3px}\
.tbar{position:absolute;top:1px;height:calc(100% - 2px);min-width:2px;border-radius:2px;opacity:.9}\
.tbar.pass{background:var(--pass)}.tbar.fail{background:var(--fail)}.tbar.skip{background:var(--skip)}.tbar.warn{background:var(--warn)}\
ol.slow{list-style:none;counter-reset:s;margin:0 0 1.5rem;padding:0}\
ol.slow li{counter-increment:s;display:flex;align-items:center;gap:.6rem;margin:.3rem 0;font-size:.9rem}\
ol.slow li::before{content:counter(s);color:var(--muted);font-size:.75rem;min-width:1.2rem;text-align:right;font-variant-numeric:tabular-nums}\
ol.slow a{color:inherit;flex:0 1 auto;max-width:22rem;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}\
.slow-bar{flex:1;min-width:3rem;height:.55rem;background:var(--card);border:1px solid var(--line);border-radius:3px;overflow:hidden}\
.slow-ms{color:var(--muted);font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.8rem;font-variant-numeric:tabular-nums;min-width:5rem;text-align:right}\
.detail{background:var(--bg);border:1px solid var(--line);border-radius:6px;padding:.5rem .7rem;margin:.4rem 0 0;white-space:pre-wrap;font-size:.82rem;overflow-x:auto}\
.via{color:var(--muted);font-size:.78rem;font-family:ui-monospace,SFMono-Regular,Menlo,monospace;margin:.3rem 0 0}\
";

#[cfg(test)]
mod style_tests {
    #![allow(clippy::unwrap_used)]

    use super::STYLE;

    /// Relative luminance, WCAG 2.1 §relative-luminance.
    fn luminance(hex: &str) -> f64 {
        let channel = |s: &str| {
            let v = f64::from(u8::from_str_radix(s, 16).unwrap()) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(&hex[1..3]) + 0.7152 * channel(&hex[3..5]) + 0.0722 * channel(&hex[5..7])
    }

    fn contrast(a: &str, b: &str) -> f64 {
        let (first, second) = (luminance(a), luminance(b));
        let (lighter, darker) = (first.max(second), first.min(second));
        (lighter + 0.05) / (darker + 0.05)
    }

    /// Every `--token:#rrggbb` in one `{…}` block, with `#rgb` expanded.
    fn tokens(block: &str) -> std::collections::BTreeMap<String, String> {
        let mut out = std::collections::BTreeMap::new();
        // Braces separate declarations as far as this scan is concerned: the
        // first `--token` in a block is glued to its selector (`:root{--bg:…`),
        // and splitting on `;` alone silently drops it — which it did, leaving
        // `--bg` absent from *both* palettes and the contrast assertion
        // comparing against nothing.
        let flattened = block.replace(['{', '}'], ";");
        for decl in flattened.split(';') {
            let Some((name, value)) = decl.split_once(':') else {
                continue;
            };
            let Some(name) = name.trim().strip_prefix("--") else {
                continue;
            };
            let value = value.trim();
            if !value.starts_with('#') {
                continue;
            }
            let full = if value.len() == 4 {
                let mut s = String::from("#");
                for ch in value[1..].chars() {
                    s.push(ch);
                    s.push(ch);
                }
                s
            } else {
                value.to_owned()
            };
            if full.len() == 7 {
                out.insert(name.to_owned(), full);
            }
        }
        out
    }

    /// The two palettes: the bare `:root` block and the one inside the
    /// `prefers-color-scheme: dark` media query.
    fn palettes() -> (
        std::collections::BTreeMap<String, String>,
        std::collections::BTreeMap<String, String>,
    ) {
        let (light, rest) = STYLE.split_once("@media").unwrap();
        (tokens(light), tokens(rest))
    }

    /// A status colour a reader cannot resolve against its own ground is a
    /// status the report does not actually communicate.
    ///
    /// `--skip` failed this at 3.45:1 for exactly one reason: it was the single
    /// token the dark block did not redefine, so a grey chosen against
    /// `#0d1117` was left carrying `.pill.skip`'s white text on white. The
    /// ratio is asserted rather than the hex, so a future palette change is
    /// free to move the colour and not free to move it below the threshold.
    #[test]
    fn every_status_colour_meets_wcag_aa_against_its_own_background() {
        const AA: f64 = 4.5;
        let (light, dark) = palettes();
        for (name, palette) in [("light", &light), ("dark", &dark)] {
            let bg = palette.get("bg").unwrap();
            for token in ["pass", "fail", "skip", "warn", "muted"] {
                let colour = palette.get(token).unwrap();
                let ratio = contrast(colour, bg);
                assert!(
                    ratio >= AA,
                    "{name}: --{token} ({colour}) on --bg ({bg}) is {ratio:.2}:1, below AA {AA}:1"
                );
            }
            // `.pill.*` paints white text on the status colour, at .72rem bold
            // — not large text, so the same threshold applies.
            let pill_fg = palette.get("pill-fg").unwrap();
            for token in ["pass", "fail", "skip", "warn"] {
                let colour = palette.get(token).unwrap();
                let ratio = contrast(pill_fg, colour);
                assert!(
                    ratio >= AA,
                    "{name}: .pill.{token} is {pill_fg} on {colour} at {ratio:.2}:1, below AA {AA}:1"
                );
            }
        }
    }

    /// Both palettes must define the same token set: `--skip` failing above was
    /// the visible symptom of it being *absent* from the dark block and
    /// silently inheriting a value tuned for the other ground.
    #[test]
    fn both_palettes_define_the_same_tokens() {
        let (light, dark) = palettes();
        let light_names: Vec<&String> = light.keys().collect();
        let dark_names: Vec<&String> = dark.keys().collect();
        assert_eq!(light_names, dark_names, "a token is missing from a palette");
    }
}

#[cfg(test)]
mod slowest_tests {
    #![allow(clippy::unwrap_used)]

    use super::{ScenarioBlock, StepRow, render_slowest};
    use crate::step::Status;

    fn block(name: &str, durations: &[u64]) -> ScenarioBlock {
        ScenarioBlock {
            file: "f.feature".to_owned(),
            name: name.to_owned(),
            status: Some(Status::Passed),
            steps: durations
                .iter()
                .map(|ms| StepRow {
                    line: 1,
                    text: "a step".to_owned(),
                    status: Status::Passed,
                    attempts: 1,
                    duration_ms: *ms,
                    detail: None,
                    reproduce_hint: None,
                    fragment: None,
                    label: None,
                })
                .collect(),
            ..ScenarioBlock::default()
        }
    }

    fn render(blocks: &[ScenarioBlock]) -> String {
        let mut html = String::new();
        render_slowest(&mut html, blocks);
        html
    }

    /// The question the section exists to answer: slowest first, by the *sum*
    /// of a scenario's steps, each row linking to its own block.
    #[test]
    fn scenarios_are_ranked_slowest_first_by_total_step_time() {
        let html = render(&[
            block("quick", &[10]),
            block("heavy", &[400, 600]),
            block("middling", &[200]),
        ]);
        let order: Vec<&str> = ["heavy", "middling", "quick"]
            .into_iter()
            .filter(|name| html.contains(*name))
            .collect();
        assert_eq!(order, vec!["heavy", "middling", "quick"]);
        let (first, second) = (html.find("heavy").unwrap(), html.find("middling").unwrap());
        assert!(first < second, "slowest first: {html}");
        assert!(
            html.contains("1000 ms"),
            "steps are summed, not maxed: {html}"
        );
        assert!(
            html.contains("href=\"#s-f--heavy\""),
            "rows link to blocks: {html}"
        );
    }

    /// The actionable half. A reader deciding where to spend an afternoon needs
    /// the *share*, not a column of durations.
    #[test]
    fn the_heading_reports_the_share_of_run_time_covered() {
        // 900 of 1000 ms sit in the two listed scenarios.
        let html = render(&[block("a", &[600]), block("b", &[300]), block("c", &[100])]);
        assert!(html.contains("100.0% of run time"), "all three fit: {html}");
        assert!(html.contains("3 of 3"), "{html}");
    }

    /// Nothing to rank means no section — an empty ranking is noise, and a
    /// single row is that scenario's own block repeated.
    #[test]
    fn a_run_with_nothing_to_compare_renders_no_section() {
        assert!(render(&[]).is_empty(), "no scenarios");
        assert!(
            render(&[block("only", &[5])]).is_empty(),
            "one timed scenario"
        );
        assert!(
            render(&[block("a", &[0]), block("b", &[0])]).is_empty(),
            "an untimed record (no injected durations) has nothing to rank"
        );
    }
}
