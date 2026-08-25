//! Coverage-triage tests for `src/commands/compression.rs`,
//! `src/commands/sed.rs`, and `src/commands/diff_cmd.rs` that cannot be
//! expressed as spec/comparison fixtures because they need custom
//! `ExecutionLimits` (sed output-size and cycle limits).
//!
//! Runtime behavior is intentionally not changed; actual behavior is pinned
//! by these tests.

use std::time::Duration;

use rust_bash::{ExecutionLimits, RustBashBuilder};

fn limited_shell(max_output_size: usize, max_loop_iterations: usize) -> rust_bash::RustBash {
    RustBashBuilder::new()
        .execution_limits(ExecutionLimits {
            max_output_size,
            max_loop_iterations,
            max_execution_time: Duration::from_secs(10),
            ..ExecutionLimits::agent_preset()
        })
        .build()
        .unwrap()
}

// ── sed.rs: push_output / push_output_char output-size guard ───────

#[test]
fn sed_output_size_limit_stops_output_then_interpreter_aborts() {
    // 100 lines x ~12 bytes > 100-byte output budget: sed's own push_output /
    // push_output_char guards refuse further appends once over the limit
    // (they also queue "sed: output size limit exceeded" on the command's
    // stderr), then the interpreter's own max_output_size check aborts the
    // script with a LimitExceeded error carrying the truncated size.
    let mut sh = limited_shell(100, 10_000);
    let err = sh
        .exec("seq 1 100 | sed 's/$/aaaaaaaaaa/'")
        .expect_err("output exceeds the configured max_output_size");
    let msg = err.to_string();
    assert!(msg.contains("max_output_size"), "unexpected error: {msg}");
    // The abort happened far below the ~1200 bytes an unlimited run yields,
    // proving sed's internal truncation guard fired (actual_value was 139).
    assert!(
        msg.contains("139"),
        "expected truncated actual_value in error: {msg}"
    );
}

#[test]
fn sed_output_guard_first_fires_on_exact_boundary() {
    // push_output's own error body only runs when the guard trips while
    // `output_truncated` is still false. Every line is emitted as
    // push_output(text) + push_output_char('\n'), and both guards check
    // *before* pushing, so the char guard normally wins the race. The string
    // guard only fires first when a string push lands output exactly on the
    // limit: the char check passes (len == max, not >), the char push
    // crosses to max+1, and the next push_output trips with truncated still
    // false. 100-char lines with a 100-byte budget hit that alignment.
    let wide = format!("{0}\n{0}\n", "w".repeat(100));
    let mut files = std::collections::HashMap::new();
    files.insert("/wide.txt".to_string(), wide.into_bytes());
    let mut sh = RustBashBuilder::new()
        .files(files)
        .execution_limits(ExecutionLimits {
            max_output_size: 100,
            max_loop_iterations: 10_000,
            max_execution_time: Duration::from_secs(10),
            ..ExecutionLimits::agent_preset()
        })
        .build()
        .unwrap();
    let err = sh
        .exec("sed 's/z/z/' /wide.txt")
        .expect_err("output exceeds the configured max_output_size");
    assert!(
        err.to_string().contains("max_output_size"),
        "unexpected error: {err}"
    );
}

// ── sed.rs: execute_commands cycle guard ───────────────────────────

#[test]
fn sed_branch_loop_hits_cycle_limit() {
    // `:a;ba` is an infinite branch loop; the per-command cycle counter
    // aborts at max_loop_iterations.
    let mut sh = limited_shell(10 * 1024 * 1024, 100);
    let r = sh.exec("printf 'x\\n' | sed ':a;ba'").unwrap();
    assert!(
        r.stderr.contains("sed: cycle limit exceeded"),
        "stderr: {:?}",
        r.stderr
    );
    // Like q, the abort flushes the in-flight pattern space; the pipeline's
    // exit code is sed's (0 — the limit is only reported on stderr).
    assert_eq!(r.stdout, "x\n");
    assert_eq!(r.exit_code, 0);
}
