//! BWK awk conformance suite.
//!
//! Vendored `p.*`/`t.*` programs from the One True Awk testdir (see
//! `tests/fixtures/awk_conformance/README.md`) run **in-process** through
//! RustBash with inputs staged in the VFS, compared against expectations
//! recorded from GNU Awk 5.0 (`expected.toml`). Platform-independent: no
//! external awk, no host fs access at test time.
//!
//! The skip list is reverse-asserted: a skipped case that starts PASSING
//! fails the suite, so fixed gaps are promoted deliberately (and their
//! skip-list entry deleted). Divergences behind skip-list entries belong
//! in `docs/guidebook/11-known-divergences.md`.

use std::path::PathBuf;

use rust_bash::{ExecutionLimits, RustBashBuilder};
use serde::Deserialize;

#[derive(Deserialize)]
struct Expected {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    exit_code: i32,
    stdout: String,
    #[allow(dead_code)]
    stderr: String,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/awk_conformance/bwk")
}

/// Cases we do not match yet, with the reason. A case listed here that
/// starts passing is reported as an unexpected pass (remove the entry and
/// register/delete the divergence note as appropriate).
const SKIP: &[(&str, &str)] = &[
    // awk pipe forms are deliberately unsupported (policy decision — they
    // are a backdoor to command execution; they fail visibly instead).
    ("p.48", "print | \"sort\" pipe"),
    ("p.50", "print | \"sort\" pipe"),
    ("t.in", "print | pipe + close(pipe)"),
    ("t.in1", "print | pipe"),
    ("t.pipe", "\"cmd\" | getline pipe"),
    // rand() without srand() uses an implementation-defined default
    // sequence; gawk's differs from ours.
    ("p.48b", "rand() default sequence differs from gawk"),
    ("t.randk", "rand() default sequence differs from gawk"),
    // for-in iteration order is explicitly unspecified in POSIX; gawk's
    // hash order differs from ours (content matches, order does not).
    ("p.43", "for-in iteration order (unspecified)"),
    ("t.in2", "for-in iteration order (unspecified)"),
    ("t.intest2", "for-in iteration order (unspecified)"),
    // %c of a large numeric code: gawk emits the raw low byte (non-UTF-8);
    // our text-channel output is UTF-8. Byte-level corner.
    ("t.printf2", "%c raw-byte output vs UTF-8 text channel"),
];

#[test]
fn bwk_conformance() {
    let dir = fixtures_dir();
    let expected_raw = std::fs::read_to_string(dir.join("expected.toml"))
        .unwrap()
        .replace("\r\n", "\n");
    let expected: Expected = toml::from_str(&expected_raw).unwrap();
    let test_data = std::fs::read(dir.join("test.data")).unwrap();
    let test_countries = std::fs::read(dir.join("test.countries")).unwrap();

    let skip: std::collections::HashMap<&str, &str> = SKIP.iter().copied().collect();
    let mut failures = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut skipped = 0;
    let mut passed = 0;

    for case in &expected.cases {
        let program = std::fs::read_to_string(dir.join(&case.name)).unwrap();
        let is_p = case.name.starts_with("p.");

        let mut sh = RustBashBuilder::new()
            .execution_limits(ExecutionLimits {
                max_loop_iterations: 1_000_000,
                max_output_size: 16 * 1024 * 1024,
                ..Default::default()
            })
            .build()
            .unwrap();
        sh.write_file("/test.data", &test_data).unwrap();
        sh.write_file("/test.countries", &test_countries).unwrap();
        sh.write_file(&format!("/prog_{}", case.name), program.as_bytes())
            .unwrap();
        // Relative input paths so FILENAME/ARGV match the recording.
        let script = if is_p {
            format!("awk -f /prog_{} test.countries test.countries", case.name)
        } else {
            format!("awk -f /prog_{} test.data", case.name)
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sh.exec(&script)));
        let result = match result {
            Ok(r) => r,
            Err(payload) => {
                // A panic is a host bug, not a divergence — always a failure,
                // never skip-listable.
                failures.push(format!(
                    "{}: PANIC: {}",
                    case.name,
                    payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_default()
                ));
                continue;
            }
        };

        let (actual_out, actual_err, actual_rc, exec_err) = match &result {
            Ok(r) => (r.stdout.clone(), r.stderr.clone(), r.exit_code, None),
            Err(e) => (String::new(), String::new(), -1, Some(e.to_string())), // limit trips / errors
        };
        // Stderr policy: where the reference run was silent, we must be
        // silent too (catches spurious warnings); where gawk emitted an
        // error message, exact wording is a tracked-divergence concern,
        // not a conformance failure.
        let stderr_ok = !case.stderr.is_empty() || actual_err.is_empty();
        let matches = actual_out == case.stdout && actual_rc == case.exit_code && stderr_ok;

        if let Some(reason) = skip.get(case.name.as_str()) {
            if matches {
                unexpected_passes.push(format!("{} (was: {})", case.name, reason));
            } else {
                skipped += 1;
            }
            continue;
        }
        if matches {
            passed += 1;
        } else {
            let detail = if let Some(e) = &exec_err {
                format!("exec error: {e} (want rc={})", case.exit_code)
            } else {
                format!(
                    "rc: got {actual_rc} want {}; stdout differs",
                    case.exit_code
                )
            };
            failures.push(format!("{}: {detail}", case.name));
        }
    }

    println!(
        "BWK conformance: {passed} passed, {skipped} skipped, \
         {} failed, {} unexpected-pass",
        failures.len(),
        unexpected_passes.len()
    );
    if !failures.is_empty() {
        println!("failures:\n  {}", failures.join("\n  "));
    }
    assert!(
        unexpected_passes.is_empty(),
        "unexpected passes (remove skip-list entries):\n  {}",
        unexpected_passes.join("\n  ")
    );
    assert!(
        failures.is_empty(),
        "{} BWK conformance failures",
        failures.len()
    );
}
