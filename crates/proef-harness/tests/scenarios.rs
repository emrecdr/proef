//! One libtest-mimic `Trial` per proef scenario (US-12). See the crate docs
//! for configuration. Listing uses `proef flows --output json`; execution
//! uses `proef test --scenario <name>` — the nextest contract
//! (`--list --format terse`, `--exact --nocapture`) comes from libtest-mimic.

use std::process::Command;

use libtest_mimic::{Arguments, Failed, Trial};

fn main() {
    let args = Arguments::from_args();
    libtest_mimic::run(&args, discover()).exit();
}

fn proef_bin() -> String {
    std::env::var("PROEF_BIN").unwrap_or_else(|_| "proef".to_owned())
}

fn discover() -> Vec<Trial> {
    let Ok(suite) = std::env::var("PROEF_HARNESS_SUITE") else {
        return Vec::new(); // unset: expose nothing (plain `cargo test` stays green)
    };
    let output = match Command::new(proef_bin())
        .args(["flows", &suite, "--output", "json"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            return vec![Trial::test("proef::flows", move || {
                Err(Failed::from(format!("cannot list scenarios:\n{stderr}")))
            })];
        }
        Err(err) => {
            let message = format!("cannot invoke proef (set PROEF_BIN): {err}");
            return vec![Trial::test("proef::flows", move || {
                Err(Failed::from(message.clone()))
            })];
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
        let (suite, scenario) = (suite.clone(), name.to_owned());
        trials.push(Trial::test(trial_name, move || {
            run_scenario(&suite, &scenario)
        }));
    }
    trials
}

fn run_scenario(suite: &str, scenario: &str) -> Result<(), Failed> {
    let output = Command::new(proef_bin())
        .args(["test", suite, "--scenario", scenario])
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
