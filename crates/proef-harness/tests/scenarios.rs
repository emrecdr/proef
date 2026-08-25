//! One libtest-mimic `Trial` per proef scenario (US-12). See the crate docs
//! for configuration. Listing uses `proef flows --format json`; execution
//! uses `proef test --scenario <name> --scenario-file <file>` — file-scoped so
//! duplicate scenario names across features keep the Trial↔scenario bijection
//! — and the nextest contract (`--list --format terse`, `--exact
//! --nocapture`) comes from libtest-mimic.

use std::process::Command;

use libtest_mimic::{Arguments, Failed, Trial};

fn main() {
    let args = Arguments::from_args();
    libtest_mimic::run(&args, discover()).exit();
}

fn proef_bin() -> Result<String, String> {
    // A set-but-unreadable PROEF_BIN must not fall back to `proef` on PATH:
    // that silently runs a different binary than the one the caller named.
    Ok(proef_harness::env_var("PROEF_BIN")?.unwrap_or_else(|| "proef".to_owned()))
}

/// A fault that stops the harness listing real trials surfaces as one named
/// failing trial — never as an empty list, which would report green having run
/// nothing. The name says which fault, so CI output stays greppable by cause.
fn failing_trial(name: &'static str, message: String) -> Trial {
    Trial::test(name, move || Err(Failed::from(message)))
}

fn discover() -> Vec<Trial> {
    // Unset means "expose nothing" on purpose — it keeps plain workspace test
    // runs green without a fixture. Set-but-unreadable is the opposite: the
    // caller asked for a suite and we cannot read which, so exposing nothing
    // would report green having tested nothing at all.
    let suite = match proef_harness::env_var("PROEF_HARNESS_SUITE") {
        Ok(Some(suite)) => suite,
        Ok(None) => return Vec::new(),
        Err(message) => return vec![failing_trial("proef::config", message)],
    };
    let bin = match proef_bin() {
        Ok(bin) => bin,
        Err(message) => return vec![failing_trial("proef::config", message)],
    };
    let output = match Command::new(&bin)
        .args(["flows", &suite, "--format", "json"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            return vec![failing_trial(
                "proef::flows",
                format!("cannot list scenarios:\n{stderr}"),
            )];
        }
        Err(err) => {
            let message = format!("cannot invoke proef (set PROEF_BIN): {err}");
            return vec![failing_trial("proef::flows", message)];
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut trials = Vec::new();
    for line in stdout.lines() {
        let Ok(flow) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let (Some(file), Some(name)) = (flow["file"].as_str(), flow["name"].as_str()) else {
            continue;
        };
        let stem = std::path::Path::new(file)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let trial_name = format!("{stem}::{name}");
        let (suite, file, scenario) = (suite.clone(), file.to_owned(), name.to_owned());
        // An authored `@skip` maps to libtest's ignored flag — nextest
        // and IDEs then show it natively instead of running it to skip.
        let ignored = flow["skip"].is_string();
        trials.push(
            Trial::test(trial_name, move || run_scenario(&suite, &file, &scenario))
                .with_ignored_flag(ignored),
        );
    }
    if trials.is_empty() && !stdout.trim().is_empty() {
        // Flows succeeded and printed rows, yet none parsed into a trial —
        // contract drift between `proef flows --format json` and this harness
        // must fail CI loudly, never run zero tests green.
        return vec![failing_trial(
            "proef::flows-contract",
            "flows printed output but no line parsed into a trial (file/name fields \
             missing?) — the flows JSON contract and this harness disagree"
                .to_owned(),
        )];
    }
    trials
}

fn run_scenario(suite: &str, file: &str, scenario: &str) -> Result<(), Failed> {
    // The suite stays the `test` path (pack discovery is suite-rooted);
    // `--scenario-file` pins the name filter to this trial's feature file so
    // same-named scenarios in other files never ride along.
    let output = Command::new(proef_bin().map_err(Failed::from)?)
        .args([
            "test",
            suite,
            "--scenario",
            scenario,
            "--scenario-file",
            file,
        ])
        .output()
        .map_err(|err| Failed::from(format!("cannot invoke proef: {err}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Failed::from(format!(
            "proef exited with {:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status.code()
        )))
    }
}
