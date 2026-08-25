//! Behavioral tests for walker control flow: compound commands, loops,
//! loop limits, `(( ))` command edge cases, and interactive error
//! recovery, driven through the public `RustBash` API.

use rust_bash::{ExecutionLimits, RustBash, RustBashBuilder};

fn shell() -> RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> (String, String, i32) {
    let mut sh = shell();
    let r = sh.exec(script).unwrap();
    (r.stdout, r.stderr, r.exit_code)
}

fn run_limited(
    script: &str,
    max_loop_iterations: usize,
) -> Result<String, rust_bash::RustBashError> {
    let limits = ExecutionLimits {
        max_loop_iterations,
        ..ExecutionLimits::default()
    };
    let mut sh = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap();
    sh.exec(script).map(|r| r.stdout)
}

// ── noexec ──────────────────────────────────────────────────────────

#[test]
fn noexec_skips_compound_and_simple_commands() {
    let (out, _, _) = run("set -n; if true; then echo x; fi; echo done");
    assert_eq!(out, "");
}

// ── Condition evaluation errors ─────────────────────────────────────

#[test]
fn if_condition_with_redirect_failure_short_circuits() {
    let (_, err, code) = run("if echo x > /nodir/f; then echo y; else echo n; fi");
    assert_eq!(
        err,
        "rust-bash: /nodir/f: No such file or directory: /nodir\n"
    );
    assert_eq!(code, 1);
}

#[test]
fn while_condition_with_redirect_failure_breaks_loop() {
    let (out, err, _) = run("while echo x > /nodir/f; do :; done; echo done");
    assert_eq!(out, "done\n");
    assert_eq!(
        err,
        "rust-bash: /nodir/f: No such file or directory: /nodir\n"
    );
}

#[test]
fn elif_condition_with_redirect_failure_short_circuits() {
    let (out, err, _) =
        run("if false; then :; elif echo x > /nodir/f; then echo y; fi; echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(
        err,
        "rust-bash: /nodir/f: No such file or directory: /nodir\n"
    );
}

// ── Loop iteration limits ───────────────────────────────────────────

#[test]
fn for_loop_iteration_limit() {
    let r = run_limited("for i in 1 2 3 4 5; do echo $i; done", 3);
    assert!(matches!(
        r,
        Err(rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_loop_iterations",
            limit_value: 3,
            actual_value: 4,
        })
    ));
}

#[test]
fn arithmetic_for_iteration_limit() {
    let r = run_limited("for ((i=0;i<10;i++)); do echo $i; done", 3);
    assert!(matches!(
        r,
        Err(rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_loop_iterations",
            ..
        })
    ));
}

#[test]
fn while_loop_iteration_limit() {
    let r = run_limited("i=0; while true; do i=$((i+1)); done", 3);
    assert!(matches!(
        r,
        Err(rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_loop_iterations",
            ..
        })
    ));
}

// ── break/continue/return propagation ───────────────────────────────

#[test]
fn arithmetic_for_breaks_on_exit_in_body() {
    let (_, _, code) = run("for ((i=0;i<5;i++)); do exit 3; done; echo done");
    assert_eq!(code, 3);
}

#[test]
fn arithmetic_for_break_2() {
    let (out, _, _) =
        run("for ((i=0;i<3;i++)); do for ((j=0;j<3;j++)); do break 2; done; done; echo done");
    assert_eq!(out, "done\n");
}

#[test]
fn arithmetic_for_continue_2() {
    let (out, _, _) = run(
        "for ((i=0;i<3;i++)); do for ((j=0;j<3;j++)); do continue 2; echo no; done; \
         echo inner$i; done",
    );
    assert_eq!(out, "");
}

#[test]
fn arithmetic_for_return_inside_function() {
    let (out, _, _) = run("f() { for ((i=0;i<3;i++)); do return 7; done; }; f; echo rc=$?");
    assert_eq!(out, "rc=7\n");
}

#[test]
fn while_continue_2() {
    let (out, _, _) = run(
        "while false; do :; done; for i in 1 2; do while true; do continue 2; done; \
         echo no; done; echo done",
    );
    assert_eq!(out, "done\n");
}

#[test]
fn while_return_inside_function() {
    let (out, _, _) = run("f() { while true; do return 2; done; }; f; echo rc=$?");
    assert_eq!(out, "rc=2\n");
}

#[test]
fn break_inside_and_or_chain_stops_chain() {
    let (out, _, _) = run("for i in 1; do true && break && echo x; done; echo done");
    assert_eq!(out, "done\n");
}

#[test]
fn break_propagates_out_of_function_call() {
    let (out, _, _) = run("f() { break; }; for i in 1 2; do f; echo after$i; done; echo done");
    assert_eq!(out, "done\n");
}

// ── case ────────────────────────────────────────────────────────────

#[test]
fn case_nocasematch_glob() {
    let (out, _, _) = run("shopt -u extglob; shopt -s nocasematch; case A in a) echo m;; esac");
    assert_eq!(out, "m\n");
    let (out, _, _) = run("shopt -u extglob; shopt -s nocasematch; case AB in a*) echo m;; esac");
    assert_eq!(out, "m\n");
}

#[test]
fn case_plain_glob_match() {
    let (out, _, _) = run("shopt -u extglob; case A in a) echo no;; A) echo yes;; esac");
    assert_eq!(out, "yes\n");
}

// ── Function shadowing and --help interception ──────────────────────

#[test]
fn function_shadows_builtin_for_help_flag() {
    let (out, _, _) = run("echo() { printf 'mine\\n'; }; echo --help");
    assert_eq!(out, "mine\n");
}

// ── (( )) command edge cases ────────────────────────────────────────

#[test]
fn arithmetic_command_multiline_paren_ambiguity() {
    // `(( ... ) )` spanning multiple lines is reinterpreted as a subshell
    // wrapping a brace group when arithmetic evaluation fails.
    let (out, _, _) = run("((\necho hi\n) )");
    assert_eq!(out, "hi\n");
}

#[test]
fn arithmetic_command_multiline_paren_ambiguity_no_trailing_newline() {
    let (out, _, _) = run("((\necho hi) ); echo rc=$?");
    assert_eq!(out, "hi\nrc=0\n");
}

#[test]
fn arithmetic_command_multiline_assignment() {
    let (out, _, _) = run("((\nx=5\n) ); echo $x");
    assert_eq!(out, "5\n");
}

#[test]
fn arithmetic_command_invalid_hash_forms() {
    // A `#` that is not a base-N literal separator is rejected up front.
    for script in [
        "(( 1#)); echo rc=$?",
        "(( x#1 )); echo rc=$?",
        "(( 1x#1 )); echo rc=$?",
        "(( x1#1 )); echo rc=$?",
    ] {
        let (out, err, _) = run(script);
        assert_eq!(out, "rc=1\n", "script: {script}");
        assert_eq!(
            err, "rust-bash: arithmetic: unexpected character `#`\n",
            "script: {script}"
        );
    }
}

#[test]
fn arithmetic_command_valid_base_literal() {
    let (out, err, _) = run("(( 16#ff )); echo rc=$?");
    assert_eq!(out, "rc=0\n");
    assert_eq!(err, "");
}

#[test]
fn arithmetic_command_escaped_hash_reaches_evaluator() {
    // `\#` is skipped by the raw hash scan; evaluation then fails on the
    // backslash. Pinned actual behavior.
    let (out, err, _) = run("(( 1 \\# 2 )); echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(
        err,
        "rust-bash: execution error: arithmetic: unexpected character `\\`\n"
    );
}

#[test]
fn arithmetic_command_hash_inside_double_quotes() {
    let (out, err, _) = run("(( \"a\\#b\" )); echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(
        err,
        "rust-bash: execution error: arithmetic: unexpected character `\\`\n"
    );
}

#[test]
fn arithmetic_command_hash_inside_single_quotes() {
    let (out, err, _) = run("(( 'a#b' )); echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(
        err,
        "rust-bash: execution error: arithmetic: unexpected character `#`\n"
    );
}

#[test]
fn arithmetic_command_single_quote_normalization() {
    let (out, _, _) = run("x=1; (( 'x' == \"x\" )); echo rc=$?");
    assert_eq!(out, "rc=0\n");
}

#[test]
fn arithmetic_command_escaped_quote_in_double_quotes() {
    // The escaped quote survives normalization and then fails inside the
    // arithmetic tokenizer's quoted-string handling. Pinned actual behavior.
    let (out, err, _) = run("x=1; (( 'x' == \"a\\\"b\" )); echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(
        err,
        "rust-bash: execution error: arithmetic: unexpected character `\\`\n"
    );
}

#[test]
fn arithmetic_command_empty_body() {
    let (out, err, _) = run("(( )); echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(err, "");
}

#[test]
fn arithmetic_command_base_literal_at_string_start() {
    let (out, err, _) = run("((1#2)); echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(
        err,
        "rust-bash: execution error: arithmetic: invalid arithmetic base: 1\n"
    );
}

#[test]
fn arithmetic_for_without_initializer_or_updater() {
    let (out, _, _) = run("i=0; for ((;i<2;i++)); do echo $i; done");
    assert_eq!(out, "0\n1\n");
    let (out, _, _) = run("for ((i=0;i<2;)); do echo $i; break; done");
    assert_eq!(out, "0\n");
}

// ── Interactive-mode error recovery ─────────────────────────────────

#[test]
fn interactive_unbound_variable_skips_rest_of_line() {
    let (out, err, code) = run("sh -ic 'set -u; echo $undef; echo after; echo onnextline'");
    assert_eq!(out, "");
    assert_eq!(err, "rust-bash: undef: unbound variable\n");
    assert_eq!(code, 1);
}

#[test]
fn interactive_unbound_variable_continues_on_next_line() {
    let (out, err, code) = run("sh -ic 'set -u\necho $undef\necho after'");
    assert_eq!(out, "after\n");
    assert_eq!(err, "rust-bash: undef: unbound variable\n");
    assert_eq!(code, 0);
}

#[test]
fn interactive_unbound_arithmetic_skips_rest_of_line() {
    let (out, err, code) = run("sh -ic 'set -u; echo $(( y + 1 )); echo after; echo next'");
    assert_eq!(out, "");
    assert_eq!(err, "rust-bash: line 1: y: unbound variable\n");
    assert_eq!(code, 1);
}

// ── Abort on unresolved commands ────────────────────────────────────

#[test]
fn abort_on_unresolved_stops_pipeline() {
    let mut sh = RustBashBuilder::new()
        .abort_on_unresolved_commands(true)
        .build()
        .unwrap();
    let r = sh.exec("echo hi | nosuchcmd | cat; echo after").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "nosuchcmd: command not found\n");
    assert_eq!(r.exit_code, 127);
    assert_eq!(r.unresolved_commands, vec!["nosuchcmd".to_string()]);
}

// ── Compound commands with input redirects ──────────────────────────

#[test]
fn brace_group_with_input_redirect() {
    let (out, _, _) = run("echo data > /f; { cat; } < /f");
    assert_eq!(out, "data\n");
}

#[test]
fn compound_command_with_missing_input_file() {
    let (_, err, code) = run("{ echo x; } < /f");
    assert_eq!(err, "rust-bash: /f: No such file or directory\n");
    assert_eq!(code, 1);
}

// ── time / pipefail / lastpipe / PIPESTATUS ─────────────────────────

#[test]
fn time_keyword_emits_timing() {
    let (out, err, _) = run("time echo hi");
    assert_eq!(out, "hi\n");
    // Wall-clock time varies; assert the shape of the timing report.
    assert!(err.starts_with("\nreal\t0m0.00"), "err: {err}");
    assert!(
        err.ends_with("s\nuser\t0m0.000s\nsys\t0m0.000s\n"),
        "err: {err}"
    );
}

#[test]
fn lastpipe_runs_last_stage_in_current_shell() {
    let (out, _, _) = run("shopt -s lastpipe; echo a b | read x y; echo $x-$y");
    assert_eq!(out, "a-b\n");
}

#[test]
fn pipestatus_records_stage_exit_codes() {
    let (out, _, _) = run("false | true; echo ${PIPESTATUS[0]} ${PIPESTATUS[1]}");
    assert_eq!(out, "1 0\n");
}

#[test]
fn pipefail_returns_rightmost_nonzero() {
    let (out, _, _) = run("set -o pipefail; false | true; echo rc=$?");
    assert_eq!(out, "rc=1\n");
}

#[test]
fn empty_arithmetic_command_is_false() {
    // `(())` parses as an arithmetic command with empty body: the raw-text
    // `#`-detection guard treats it as having no inner expression.
    let (out, err, code) = run("(())");
    assert_eq!((out.as_str(), err.as_str(), code), ("", "", 1));
    let (_, _, code) = run("(( ))");
    assert_eq!(code, 1);
}
