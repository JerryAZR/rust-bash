//! Coverage tests for `src/commands/test_cmd.rs`: the `test`/`[` command
//! expression evaluator and the shared `[[ ]]` predicate helpers.

use rust_bash::{CommandContext, CommandResult, ExecutionLimits, InMemoryFs, RustBashBuilder};
use rust_bash::{RustBash, VirtualCommand};
use std::collections::HashMap;

fn shell() -> RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> (String, String, i32) {
    let mut sh = shell();
    let r = sh.exec(script).unwrap();
    (r.stdout, r.stderr, r.exit_code)
}

fn exit_code(script: &str) -> i32 {
    run(script).2
}

// ── 3-argument -a / -o shortcuts ──────────────────────────────────

#[test]
fn three_arg_and_shortcut_requires_both_non_empty() {
    assert_eq!(exit_code("test foo -a bar"), 0);
    assert_eq!(exit_code("test foo -a ''"), 1);
    assert_eq!(exit_code("test '' -a ''"), 1);
}

#[test]
fn three_arg_or_shortcut_requires_either_non_empty() {
    assert_eq!(exit_code("test foo -o bar"), 0);
    assert_eq!(exit_code("test foo -o ''"), 0);
    assert_eq!(exit_code("test '' -o ''"), 1);
}

// ── Operator arity errors ─────────────────────────────────────────

#[test]
fn trailing_and_operator_is_an_error() {
    let (stdout, stderr, code) = run("test foo -a");
    assert_eq!(code, 2);
    assert_eq!(stdout, "");
    assert_eq!(stderr, "test: argument expected\n");
}

#[test]
fn lone_bang_is_true_by_posix_single_arg_rule() {
    assert_eq!(exit_code("test !"), 0);
}

#[test]
fn unclosed_parenthesis_after_bang_is_an_error() {
    let (stdout, stderr, code) = run(r"test ! \( foo");
    assert_eq!(code, 2);
    assert_eq!(stdout, "");
    assert_eq!(stderr, "test: missing ')'\n");
}

// ── test -v with array subscripts ─────────────────────────────────

#[test]
fn v_flag_quoted_assoc_subscript() {
    assert_eq!(exit_code("declare -A m; m[k]=1; test -v 'm[\"k\"]'"), 0);
    assert_eq!(exit_code("declare -A m; m[k]=1; test -v \"m['k']\""), 0);
    assert_eq!(exit_code("declare -A m; m[k]=1; test -v 'm[\"z\"]'"), 1);
}

#[test]
fn v_flag_negative_indexed_subscript_counts_from_end() {
    assert_eq!(exit_code("arr=(a b c); test -v 'arr[-1]'"), 0);
    assert_eq!(exit_code("arr=(a b c); test -v 'arr[-3]'"), 0);
    // Resolves before the start of the array.
    assert_eq!(exit_code("arr=(a b c); test -v 'arr[-4]'"), 1);
}

#[test]
fn v_flag_scalar_subscript_only_index_zero_counts() {
    assert_eq!(exit_code("s=hi; test -v 's[0]'"), 0);
    assert_eq!(exit_code("s=hi; test -v 's[1]'"), 1);
    assert_eq!(exit_code("e=''; test -v 'e[0]'"), 1);
}

#[test]
fn v_flag_subscript_on_unset_variable_is_false() {
    assert_eq!(exit_code("test -v 'nope[0]'"), 1);
}

#[test]
fn v_flag_subscript_index_may_be_a_variable() {
    assert_eq!(exit_code("i=1; arr=(a b c); test -v 'arr[i]'"), 0);
    assert_eq!(exit_code("i=9; arr=(a b c); test -v 'arr[i]'"), 1);
}

#[test]
fn v_flag_subscript_index_supports_simple_arithmetic() {
    assert_eq!(exit_code("i=2; arr=(a b c); test -v 'arr[i-1]'"), 0);
    assert_eq!(exit_code("i=1; arr=(a b c); test -v 'arr[i*2]'"), 0);
    // Unsupported operators fall back to index 0.
    assert_eq!(exit_code("i=1; arr=(a b c); test -v 'arr[i/1]'"), 0);
}

// ── File comparison operators ─────────────────────────────────────

#[test]
fn ef_false_when_right_does_not_exist() {
    assert_eq!(exit_code("test /bin -ef /nonexistent"), 1);
}

#[test]
fn nt_true_when_only_left_exists() {
    assert_eq!(exit_code("test /bin -nt /nonexistent"), 0);
}

#[test]
fn bracket_bracket_ef_with_relative_paths() {
    assert_eq!(
        exit_code("cd /tmp; touch same_a; [[ same_a -ef same_a ]]"),
        0
    );
    assert_eq!(
        exit_code("cd /tmp; touch same_b; [[ same_b -ef missing_b ]]"),
        1
    );
}

#[test]
fn bracket_bracket_nt_true_when_only_left_exists() {
    assert_eq!(
        exit_code("cd /tmp; touch newer_a; [[ newer_a -nt missing_nt ]]"),
        0
    );
}

#[test]
fn bracket_bracket_ot_true_when_only_right_exists() {
    assert_eq!(
        exit_code("cd /tmp; touch older_a; [[ missing_ot -ot older_a ]]"),
        0
    );
}

#[test]
fn bracket_bracket_ot_false_when_neither_exists() {
    assert_eq!(exit_code("[[ missing_ot1 -ot missing_ot2 ]]"), 1);
}

// ── [[ -o optname ]] shell option tests ───────────────────────────

#[test]
fn bracket_bracket_o_reports_enabled_options() {
    assert_eq!(exit_code("set -e; [[ -o errexit ]]"), 0);
    assert_eq!(exit_code("set -u; [[ -o nounset ]]"), 0);
    assert_eq!(exit_code("set -o pipefail; [[ -o pipefail ]]"), 0);
    assert_eq!(exit_code("set -o xtrace; [[ -o xtrace ]]"), 0);
    assert_eq!(exit_code("set -o verbose; [[ -o verbose ]]"), 0);
    assert_eq!(exit_code("set -C; [[ -o noclobber ]]"), 0);
    assert_eq!(exit_code("set -a; [[ -o allexport ]]"), 0);
    assert_eq!(exit_code("set -f; [[ -o noglob ]]"), 0);
    assert_eq!(exit_code("set -o posix; [[ -o posix ]]"), 0);
    assert_eq!(exit_code("set -o vi; [[ -o vi ]]"), 0);
    assert_eq!(exit_code("set -o emacs; [[ -o emacs ]]"), 0);
}

#[test]
fn bracket_bracket_o_reports_disabled_options() {
    assert_eq!(exit_code("[[ -o errexit ]]"), 1);
    assert_eq!(exit_code("[[ -o noexec ]]"), 1);
    assert_eq!(exit_code("[[ -o nosuchopt ]]"), 1);
}

#[test]
fn bracket_bracket_o_errtrace_tracks_errexit_flag() {
    // rust-bash maps `-o errtrace` onto the errexit flag; `set -o errtrace`
    // does not set errexit, so this is false.
    // Suspected divergence: bash tracks errtrace (set -E) as a distinct option.
    assert_eq!(exit_code("[[ -o errtrace ]]"), 1);
    assert_eq!(exit_code("set -o errtrace; [[ -o errtrace ]]"), 1);
    assert_eq!(exit_code("set -e; [[ -o errtrace ]]"), 0);
}

// ── bash integer literals in [[ ]] arithmetic predicates ─────────

#[test]
fn bracket_bracket_arithmetic_accepts_leading_plus() {
    assert_eq!(exit_code("[[ +5 -eq 5 ]]"), 0);
}

#[test]
fn bracket_bracket_arithmetic_base_n_literals() {
    assert_eq!(exit_code("[[ 62#Z -eq 61 ]]"), 0);
    assert_eq!(exit_code("[[ 64#@ -eq 62 ]]"), 0);
    assert_eq!(exit_code("[[ 64#_ -eq 63 ]]"), 0);
}

#[test]
fn bracket_bracket_arithmetic_rejects_out_of_range_base() {
    // 65 is not a valid base (2..=64). rust-bash falls back to treating the
    // operand as 0, so the comparison is false.
    // Suspected divergence: bash reports "value too great for base" on stderr.
    assert_eq!(exit_code("[[ 65#a -eq 5 ]]"), 1);
    assert_eq!(exit_code("[[ 1#a -eq 5 ]]"), 1);
}

#[test]
fn bracket_bracket_arithmetic_rejects_digit_not_in_base() {
    // Digit 2 is not valid in base 2; the operand falls back to 0.
    assert_eq!(exit_code("[[ 2#2 -eq 2 ]]"), 1);
    assert_eq!(exit_code("[[ 16#g -eq 16 ]]"), 1);
}

#[test]
fn bracket_bracket_arithmetic_invalid_literal_becomes_zero() {
    // `.` is not a valid base-N digit, so parsing fails and the operand
    // silently becomes 0, making `0 -eq 0` true.
    // Suspected divergence: bash aborts with "syntax error in expression"
    // and exit code 1.
    assert_eq!(exit_code("[[ 2#1.1 -eq 0 ]]"), 0);
    assert_eq!(exit_code("[[ 8#! -eq 0 ]]"), 0);
}

// ── Direct command invocation (no interpreter variable context) ───

fn run_test_command(args: &[&str], env: &HashMap<String, String>) -> CommandResult {
    let fs = InMemoryFs::new();
    let limits = ExecutionLimits::default();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let ctx = CommandContext {
        fs: &fs,
        cwd: "/",
        env,
        variables: None,
        stdin: "",
        stdin_bytes: None,
        limits: &limits,
        exec: None,
        shell_opts: None,
    };
    rust_bash::commands::TestCommand.execute(&args, &ctx)
}

#[test]
fn v_flag_falls_back_to_env_without_variable_context() {
    let mut env = HashMap::new();
    env.insert("FOO".to_string(), "bar".to_string());

    // Hosts may invoke the command directly without an interpreter variable
    // map; `test -v` then consults the plain environment.
    assert_eq!(run_test_command(&["-v", "FOO"], &env).exit_code, 0);
    assert_eq!(run_test_command(&["-v", "MISSING"], &env).exit_code, 1);
}

#[test]
fn o_flag_is_false_without_shell_opts_context() {
    let env = HashMap::new();
    // No shell-option context: every option reads as disabled.
    assert_eq!(run_test_command(&["-o", "errexit"], &env).exit_code, 1);
}

#[test]
fn v_flag_subscript_without_variable_context_checks_env_literally() {
    let mut env = HashMap::new();
    env.insert("arr[0]".to_string(), "x".to_string());

    // Without a variable map, a subscripted operand is looked up verbatim in
    // the plain environment.
    assert_eq!(run_test_command(&["-v", "arr[0]"], &env).exit_code, 0);
    let empty = HashMap::new();
    assert_eq!(run_test_command(&["-v", "arr[0]"], &empty).exit_code, 1);
}

// ── Non-standard operators ────────────────────────────────────────

#[test]
fn tilde_equals_is_always_false_in_test_command() {
    // rust-bash parses `=~` as a binary operator but always evaluates it to
    // false.
    // Suspected divergence: bash rejects it outright
    // ("bash: test: =~: binary operator expected", exit code 2).
    assert_eq!(exit_code("test foo =~ bar"), 1);
}
