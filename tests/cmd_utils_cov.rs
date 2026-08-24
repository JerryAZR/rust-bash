//! Coverage-triage tests for the command layer:
//! `src/commands/utils.rs`, `src/commands/mod.rs`, `src/commands/navigation.rs`,
//! and `src/commands/regex_util.rs`.
//!
//! Every test here exists to cover a previously-uncovered region of one of
//! those files: untested flag combinations, error paths, and edge-case
//! operands. Where behavior diverges from real bash/GNU coreutils, the actual
//! behavior is pinned with a comment (runtime behavior is intentionally not
//! changed).

use std::sync::Arc;
use std::time::Duration;

use rust_bash::{
    CommandContext, CommandResult, ExecResult, ExecutionLimits, RustBash, RustBashBuilder,
    RustBashError, VirtualCommand,
};

fn shell() -> RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> ExecResult {
    shell().exec(script).unwrap()
}

// ── mod.rs: VirtualCommand::meta() default (lines 131-133) ─────────

struct NoMetaCommand;

impl VirtualCommand for NoMetaCommand {
    fn name(&self) -> &str {
        "nometa"
    }

    // meta() intentionally not overridden: exercises the default `None`.

    fn execute(&self, _args: &[String], _ctx: &CommandContext) -> CommandResult {
        CommandResult {
            stdout: "nometa ran\n".into(),
            ..Default::default()
        }
    }
}

#[test]
fn command_without_meta_help_falls_through_to_dispatch() {
    let mut sh = RustBashBuilder::new()
        .command(Arc::new(NoMetaCommand))
        .build()
        .unwrap();
    // `nometa --help`: check_help finds no metadata (default meta() -> None)
    // and falls through to normal dispatch.
    let r = sh.exec("nometa --help").unwrap();
    assert_eq!(r.stdout, "nometa ran\n");
    assert_eq!(r.exit_code, 0);
}

// ── mod.rs: echo flag/escape edge cases ─────────────────────────────

#[test]
fn echo_capital_e_disables_escapes() {
    // Covers the 'E' flag arm: -E disables escape interpretation.
    let r = run("echo -E 'a\\nb'");
    assert_eq!(r.stdout, "a\\nb\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn echo_unicode_escape_without_digits_is_literal() {
    // \u / \U with no following hex digits are emitted literally.
    let r = run("echo -e 'a\\uZ'");
    assert_eq!(r.stdout, "a\\uZ\n");
    let r = run("echo -e 'x\\U!'");
    assert_eq!(r.stdout, "x\\U!\n");
}

// ── mod.rs: cat ─────────────────────────────────────────────────────

#[test]
fn cat_unknown_flag_is_ignored() {
    let r = run("printf 'hi\\n' > /f; cat -B /f");
    assert_eq!(r.stdout, "hi\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn cat_number_lines_without_trailing_newline() {
    // Content not ending in '\n' takes the lines.len() branch.
    let r = run("printf 'a\\nb' > /f; cat -n /f");
    assert_eq!(r.stdout, "     1\ta\n     2\tb");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn cat_binary_content_uses_marker_bytes() {
    // Non-UTF8 bytes decode to private-use marker chars, so cat takes the
    // stdout_bytes branch internally (the top-level ExecResult does not
    // surface stdout_bytes; the marker char in stdout proves the branch).
    let r = run("printf '\\xff' > /f; cat /f");
    assert_eq!(r.stdout, "\u{e0ff}");
    assert_eq!(r.exit_code, 0);
}

// ── mod.rs: touch / mkdir error paths ───────────────────────────────

#[test]
fn touch_missing_operand() {
    let r = run("touch");
    assert_eq!(r.stderr, "touch: missing file operand\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn mkdir_unknown_flag_is_ignored() {
    let r = run("mkdir -m /newdir; test -d /newdir");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn mkdir_missing_operand() {
    let r = run("mkdir");
    assert_eq!(r.stderr, "mkdir: missing operand\n");
    assert_eq!(r.exit_code, 1);
}

// ── mod.rs: ls ──────────────────────────────────────────────────────

#[test]
fn ls_unknown_flag_char_is_ignored() {
    let r = run("mkdir /d; touch /d/f; ls -z /d");
    assert_eq!(r.stdout, "f\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn ls_multiple_targets_separated_by_blank_line() {
    let r = run("mkdir /a /b; touch /a/x /b/y; ls /a /b");
    assert_eq!(r.stdout, "/a:\nx\n\n/b:\ny\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn ls_long_format_single_file() {
    // readdir fails on a regular file; stat branch prints the long entry.
    let r = run("touch /plain.txt; ls -l /plain.txt");
    assert_eq!(r.stdout, "-rw-r--r-- /plain.txt\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn ls_long_format_directory_containing_symlink() {
    // The entry's node_type (Symlink) drives the type char in long listings.
    let r = run("mkdir /d; touch /d/t; ln -s /d/t /d/link; ls -l /d");
    assert_eq!(r.stdout, "lrw-r--r-- link\n-rw-r--r-- t\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn ls_recursive_from_dot_uses_dot_slash_headers() {
    let r = run("mkdir -p /w/sub; touch /w/sub/f; cd /w; ls -R .");
    assert_eq!(r.stdout, ".:\nsub\n\n./sub:\nf\n");
    assert_eq!(r.exit_code, 0);
}

// ── navigation.rs: realpath ─────────────────────────────────────────

#[test]
fn realpath_double_dash_and_ignored_flags() {
    let r = run("mkdir -p /a/b; touch /a/b/c.txt; realpath -- /a/b/c.txt; realpath -s /a/b/c.txt");
    assert_eq!(r.stdout, "/a/b/c.txt\n/a/b/c.txt\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn realpath_nonexistent_path() {
    let r = run("realpath /nope");
    // NOTE: pinned actual behavior — the VFS error message repeats the path
    // (GNU prints "realpath: /nope: No such file or directory").
    assert_eq!(
        r.stderr,
        "realpath: /nope: No such file or directory: /nope\n"
    );
    assert_eq!(r.exit_code, 1);
}

// ── navigation.rs: basename / dirname ───────────────────────────────

#[test]
fn basename_double_dash_and_ignored_flags() {
    let r = run("basename -- foo.txt; basename -a foo.txt");
    // NOTE: pinned actual behavior — GNU `basename -a` treats every argument
    // as a name; rust-bash silently ignores unknown flags instead.
    assert_eq!(r.stdout, "foo.txt\nfoo.txt\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn dirname_double_dash_and_ignored_flags() {
    let r = run("dirname -- /a/b; dirname -z /a/b");
    assert_eq!(r.stdout, "/a\n/a\n");
    assert_eq!(r.exit_code, 0);
}

// ── navigation.rs: tree ─────────────────────────────────────────────

#[test]
fn tree_double_dash_and_ignored_flags() {
    let r = run("mkdir /t; touch /t/f; tree -- /t; tree -a /t");
    assert_eq!(
        r.stdout,
        "/t\n└── f\n\n0 directories, 1 files\n/t\n└── f\n\n0 directories, 1 files\n"
    );
    assert_eq!(r.exit_code, 0);
}

#[test]
fn tree_on_regular_file_prints_zero_counts() {
    // The target exists, so execution enters tree_recursive, where readdir
    // fails on a non-directory and returns immediately.
    let r = run("touch /leaf; tree /leaf");
    assert_eq!(r.stdout, "/leaf\n\n0 directories, 0 files\n");
    assert_eq!(r.exit_code, 0);
}

// ── regex_util.rs via grep BRE mode ─────────────────────────────────

#[test]
fn grep_bre_escaped_question_mark_is_quantifier() {
    // In BRE, `\?` is the quantifier; covers bre_to_ere `\?` -> `?`.
    let r = run("printf 'ab\\na\\n' | grep 'ab\\?'");
    assert_eq!(r.stdout, "ab\na\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn grep_bre_bare_braces_are_literal() {
    // In BRE, bare { } are literal; covers bre_to_ere `{` -> `\{`, `}` -> `\}`.
    let r = run("printf 'a{2}\\naa\\n' | grep 'a{2}'");
    assert_eq!(r.stdout, "a{2}\n");
    assert_eq!(r.exit_code, 0);
}

// ── utils.rs: expr ──────────────────────────────────────────────────

#[test]
fn expr_substr_position_zero_is_empty() {
    let r = run("expr substr hello 0 3");
    assert_eq!(r.stdout, "\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn expr_match_keyword() {
    let r = run("expr match hello hel");
    assert_eq!(r.stdout, "3\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn expr_trailing_tokens_are_syntax_error() {
    let r = run("expr 1 + 1 2");
    assert_eq!(r.stderr, "expr: syntax error\n");
    assert_eq!(r.exit_code, 2);
}

#[test]
fn expr_or_operator() {
    let r = run("expr 0 '|' abc; expr abc '|' def");
    assert_eq!(r.stdout, "abc\nabc\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn expr_and_operator() {
    let r = run("expr abc '&' def; expr 0 '&' def");
    assert_eq!(r.stdout, "abc\n0\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn expr_numeric_comparison_operators() {
    let r = run("expr 3 '<=' 4; expr 5 '>=' 6; expr 4 '>=' 3");
    assert_eq!(r.stdout, "1\n0\n1\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn expr_string_comparison_operators() {
    let r = run(
        "expr abc = abc; expr abc = abd; expr abc '!=' abd; expr abc '<' abd; expr abc '>' abd; expr abc '<=' abc; expr abc '>=' abd",
    );
    assert_eq!(r.stdout, "1\n0\n1\n1\n0\n1\n0\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn expr_subtraction() {
    let r = run("expr 10 - 4");
    assert_eq!(r.stdout, "6\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn expr_colon_without_pattern_is_syntax_error() {
    let r = run("expr hello :");
    assert_eq!(r.stderr, "expr: syntax error\n");
    assert_eq!(r.exit_code, 2);
}

#[test]
fn expr_operator_without_rhs_is_syntax_error() {
    let r = run("expr 5 +");
    assert_eq!(r.stderr, "expr: syntax error\n");
    assert_eq!(r.exit_code, 2);
}

#[test]
fn expr_parenthesized_expression() {
    let r = run("expr '(' 1 + 2 ')' '*' 3");
    assert_eq!(r.stdout, "9\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn expr_unbalanced_paren_is_syntax_error() {
    let r = run("expr '(' 1 + 2");
    assert_eq!(r.stderr, "expr: syntax error: expecting ')'\n");
    assert_eq!(r.exit_code, 2);
}

#[test]
fn expr_match_pattern_with_caret_anchor() {
    let r = run("expr match hello '^hel'");
    assert_eq!(r.stdout, "3\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn expr_match_with_capture_group_returns_group() {
    let r = run("expr match hello 'h(e)l'");
    assert_eq!(r.stdout, "e\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn expr_match_failure_with_group_is_empty_without_group_is_zero() {
    let r = run("expr match hello 'z(z)'; expr match hello zzz");
    assert_eq!(r.stdout, "\n0\n");
    assert_eq!(r.exit_code, 1);
}

// ── utils.rs: sleep ─────────────────────────────────────────────────

#[test]
fn sleep_negative_interval_is_invalid() {
    let r = run("sleep -1");
    assert_eq!(r.stderr, "sleep: invalid time interval '-1'\n");
    assert_eq!(r.exit_code, 1);
}

// ── utils.rs: seq ───────────────────────────────────────────────────

#[test]
fn seq_double_dash_ends_options() {
    let r = run("seq -- 3");
    assert_eq!(r.stdout, "1\n2\n3\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn seq_negative_single_operand_prints_nothing() {
    // Negative number is treated as an operand, not a flag.
    let r = run("seq -3");
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn seq_invalid_arguments() {
    let r = run("seq abc");
    assert_eq!(r.stderr, "seq: invalid argument 'abc'\n");
    assert_eq!(r.exit_code, 1);
    let r = run("seq 1 abc");
    assert_eq!(r.stderr, "seq: invalid argument 'abc'\n");
    assert_eq!(r.exit_code, 1);
    let r = run("seq a 1 3");
    assert_eq!(r.stderr, "seq: invalid argument 'a'\n");
    assert_eq!(r.exit_code, 1);
    let r = run("seq 1 a 3");
    assert_eq!(r.stderr, "seq: invalid argument 'a'\n");
    assert_eq!(r.exit_code, 1);
    let r = run("seq 1 1 a");
    assert_eq!(r.stderr, "seq: invalid argument 'a'\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn seq_two_operands_first_invalid() {
    let r = run("seq a 3");
    assert_eq!(r.stderr, "seq: invalid argument 'a'\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn seq_stops_at_iteration_safety_limit() {
    // The hard-coded 1,000,000-iteration safety cap terminates the loop even
    // though more values would fit the range.
    let r = run("seq 1 1000005");
    assert_eq!(r.stdout.lines().count(), 1_000_000);
    assert_eq!(r.exit_code, 0);
}

#[test]
fn seq_zero_increment() {
    let r = run("seq 1 0 3");
    assert_eq!(r.stderr, "seq: zero increment\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn seq_descending_range() {
    let r = run("seq 3 1");
    assert_eq!(r.stdout, "3\n2\n1\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn seq_float_operands() {
    let r = run("seq 0.5 1.5");
    assert_eq!(r.stdout, "0.5\n1.5\n");
    assert_eq!(r.exit_code, 0);
}

// ── utils.rs: env ───────────────────────────────────────────────────

#[test]
fn env_unsupported_option() {
    let r = run("env -z");
    assert_eq!(r.stderr, "env: unsupported option '-z'\n");
    assert_eq!(r.exit_code, 125);
}

#[test]
fn env_invalid_assignment_names_are_treated_as_command() {
    // `=x`: empty name fails parse_env_assignment (chars.next() -> None),
    // so the token is treated as a command to execute.
    let r = run("env =x");
    assert_eq!(r.exit_code, 127);
    // `1abc=x`: leading digit is not a valid variable name.
    let r = run("env 1abc=x");
    assert_eq!(r.exit_code, 127);
    // `a-b=x`: later characters must be alphanumeric or underscore.
    let r = run("env a-b=x");
    assert_eq!(r.exit_code, 127);
}

#[test]
fn env_exec_callback_error_is_reported() {
    // The sub-interpreter exceeds max_output_size, so the exec callback
    // returns Err and env reports `env: <err>` with exit 125. The outer
    // interpreter then hits the same limit, so the whole exec() fails —
    // the inner env lines are still exercised.
    let limits = ExecutionLimits {
        max_output_size: 10,
        ..Default::default()
    };
    let mut sh = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap();
    let r = sh.exec("env seq 100");
    assert!(
        matches!(r, Err(RustBashError::LimitExceeded { .. })),
        "expected LimitExceeded, got: {r:?}"
    );
}

// ── utils.rs: which ─────────────────────────────────────────────────

#[test]
fn which_path_with_slash_existing_file() {
    let r = run("which /bin/echo");
    assert_eq!(r.stdout, "/bin/echo\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn which_relative_path_with_slash() {
    // NOTE: pinned actual behavior — the resolved path keeps the literal
    // `./` component (GNU which would print `./q`).
    let r = run("mkdir /tmp; touch /tmp/q; cd /tmp; which ./q");
    assert_eq!(r.stdout, "/tmp/./q\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn which_path_with_slash_not_found() {
    let r = run("which /nope");
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn which_directory_is_not_a_command() {
    let r = run("which /");
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 1);
}

// ── utils.rs: base64 ────────────────────────────────────────────────

#[test]
fn base64_double_dash_reads_stdin() {
    let r = run("printf 'hello' | base64 --");
    assert_eq!(r.stdout, "aGVsbG8=\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn base64_wrap_attached_value() {
    let r = run("printf 'hello world foo bar' | base64 -w4");
    assert_eq!(r.stdout, "aGVs\nbG8g\nd29y\nbGQg\nZm9v\nIGJh\ncg==\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn base64_wrap_separate_value() {
    let r = run("printf 'hello world' | base64 -w 4");
    assert_eq!(r.stdout, "aGVs\nbG8g\nd29y\nbGQ=\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn base64_wrap_flag_without_value_uses_default() {
    let r = run("printf 'hi' | base64 -w");
    assert_eq!(r.stdout, "aGk=\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn base64_wrap_zero_disables_wrapping() {
    let r = run("printf 'hello world foo bar' | base64 -w0");
    assert_eq!(r.stdout, "aGVsbG8gd29ybGQgZm9vIGJhcg==\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn base64_default_wraps_at_76_columns() {
    // 60 bytes of input encode to 80 base64 chars -> wrapped after 76.
    let r = run("printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' | base64");
    assert_eq!(
        r.stdout,
        "YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFh\nYWFh\n"
    );
    assert_eq!(r.exit_code, 0);
}

#[test]
fn base64_missing_file() {
    let r = run("base64 /nope");
    // NOTE: pinned actual behavior — the VFS error message repeats the path.
    assert_eq!(
        r.stderr,
        "base64: /nope: No such file or directory: /nope\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn base64_decode_invalid_input() {
    let r = run("printf '!!!' | base64 -d");
    assert!(
        r.stderr.starts_with("base64: invalid input: "),
        "stderr: {:?}",
        r.stderr
    );
    assert_eq!(r.exit_code, 1);
}

// ── utils.rs: checksum commands ─────────────────────────────────────

#[test]
fn md5sum_double_dash_and_ignored_flags() {
    let r = run("printf 'x' > /f; md5sum -- /f; md5sum -z /f");
    assert_eq!(
        r.stdout,
        "9dd4e461268c8034f5c8564e155c67a6  /f\n9dd4e461268c8034f5c8564e155c67a6  /f\n"
    );
    assert_eq!(r.exit_code, 0);
}

#[test]
fn sha256sum_double_dash_ignored_flags_and_missing_file() {
    let r = run("printf 'x' > /f; sha256sum -- /f; sha256sum -z /f");
    assert_eq!(
        r.stdout,
        "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881  /f\n2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881  /f\n"
    );
    let r = run("sha256sum /nope");
    // NOTE: pinned actual behavior — the VFS error message repeats the path.
    assert_eq!(
        r.stderr,
        "sha256sum: /nope: No such file or directory: /nope\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn sha1sum_double_dash_ignored_flags_and_missing_file() {
    let r = run("printf 'x' > /f; sha1sum -- /f; sha1sum -z /f");
    assert_eq!(
        r.stdout,
        "11f6ad8ec52a2984abaafd7c3b516503785c2072  /f\n11f6ad8ec52a2984abaafd7c3b516503785c2072  /f\n"
    );
    let r = run("sha1sum /nope");
    // NOTE: pinned actual behavior — the VFS error message repeats the path.
    assert_eq!(
        r.stderr,
        "sha1sum: /nope: No such file or directory: /nope\n"
    );
    assert_eq!(r.exit_code, 1);
}

// ── utils.rs: uname ─────────────────────────────────────────────────

#[test]
fn uname_individual_flags() {
    let r = run("uname -snr");
    assert_eq!(r.stdout, "Linux rust-bash 6.0.0-virtual\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn uname_unknown_flag_chars_are_ignored() {
    // NOTE: pinned actual behavior — GNU `uname -z` fails with
    // "uname: invalid option -- 'z'"; rust-bash silently ignores unknown
    // flag characters and prints an empty line.
    let r = run("uname -z");
    assert_eq!(r.stdout, "\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn uname_non_flag_operand_is_ignored() {
    // NOTE: pinned actual behavior — GNU `uname foo` fails with
    // "uname: extra operand 'foo'"; rust-bash ignores non-flag operands and
    // (with no flags selected) prints an empty line.
    let r = run("uname foo");
    assert_eq!(r.stdout, "\n");
    assert_eq!(r.exit_code, 0);
}

// ── utils.rs: timeout ───────────────────────────────────────────────

#[test]
fn timeout_skips_k_and_s_flags() {
    let r = run(
        "timeout -k 5 10 true; timeout -s KILL 10 true; timeout --kill-after=5 --signal=KILL 10 true",
    );
    assert_eq!(r.exit_code, 0);
}

#[test]
fn timeout_missing_operands() {
    let r = run("timeout -k 5");
    assert_eq!(r.stderr, "timeout: missing operand\n");
    assert_eq!(r.exit_code, 125);
    let r = run("timeout 5");
    assert_eq!(r.stderr, "timeout: missing operand\n");
    assert_eq!(r.exit_code, 125);
}

#[test]
fn timeout_invalid_interval() {
    let r = run("timeout abc true");
    assert_eq!(r.stderr, "timeout: invalid time interval 'abc'\n");
    assert_eq!(r.exit_code, 125);
}

#[test]
fn timeout_expired_returns_124() {
    // The command runs to completion and is found to have exceeded the
    // duration afterwards (the sandbox cannot preempt — see the NOTE in
    // utils.rs TimeoutCommand::execute).
    let r = run("timeout 0.05 sleep 0.2");
    assert_eq!(r.exit_code, 124);
}

#[test]
fn timeout_exec_callback_error_paths() {
    // The sub-command exceeds max_output_size, so the exec callback returns
    // Err. Elapsed time decides between the two error arms:
    //  - `timeout 5 ...`: elapsed < duration -> exit 126 arm
    //  - `timeout 0.0000001 ...`: elapsed > duration -> exit 124 arm
    // The outer interpreter then hits the same limit and exec() fails, but
    // the inner timeout arms are still exercised.
    let limits = ExecutionLimits {
        max_output_size: 10,
        ..Default::default()
    };
    let mut sh = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap();
    let r = sh.exec("timeout 5 seq 100");
    assert!(
        matches!(r, Err(RustBashError::LimitExceeded { .. })),
        "expected LimitExceeded, got: {r:?}"
    );
    let r = sh.exec("timeout 0.0000001 seq 100");
    assert!(
        matches!(r, Err(RustBashError::LimitExceeded { .. })),
        "expected LimitExceeded, got: {r:?}"
    );
}

#[test]
fn timeout_exec_error_after_expiry_via_time_limit() {
    // Variant of the expired-error arm driven by max_execution_time: the
    // sub-command is capped to the limit, then the callback returns Err
    // with elapsed > duration -> exit 124 arm.
    let limits = ExecutionLimits {
        max_execution_time: Duration::from_millis(50),
        ..Default::default()
    };
    let mut sh = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap();
    let r = sh.exec("timeout 0.0000001 sleep 5");
    assert!(
        matches!(r, Err(RustBashError::Timeout)),
        "expected Timeout, got: {r:?}"
    );
}

// ── utils.rs: file ──────────────────────────────────────────────────

#[test]
fn file_missing_operand() {
    let r = run("file");
    assert_eq!(r.stderr, "file: missing operand\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn file_double_dash_and_missing_path() {
    let r = run("printf 'hi' > /f; file -- /f");
    assert_eq!(r.stdout, "/f: ASCII text\n");
    let r = run("file /nope");
    // NOTE: pinned actual behavior — GNU prints
    // "/nope: cannot open `/nope' (No such file or directory)"; the VFS
    // error message also repeats the path.
    assert_eq!(
        r.stderr,
        "/nope: cannot open (No such file or directory: /nope)\n"
    );
    assert_eq!(r.exit_code, 1);
}

#[test]
fn file_magic_bytes() {
    let r = run("printf '\\xff\\xd8\\xff' > /f; file /f");
    assert_eq!(r.stdout, "/f: JPEG image data\n");
    let r = run("printf 'GIF89a' > /f; file /f");
    assert_eq!(r.stdout, "/f: GIF image data\n");
    let r = run("printf '\\x7fELF' > /f; file /f");
    assert_eq!(r.stdout, "/f: ELF executable\n");
    let r = run("printf '\\x1f\\x8b' > /f; file /f");
    assert_eq!(r.stdout, "/f: gzip compressed data\n");
    // NOTE: `%` must be escaped as `%%` so printf does not treat it as a
    // format directive.
    let r = run("printf '%%PDF-1.4' > /f; file /f");
    assert_eq!(r.stdout, "/f: PDF document\n");
    let r = run("printf 'PK\\x03\\x04' > /f; file /f");
    assert_eq!(r.stdout, "/f: Zip archive data\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn file_tar_archive_via_tar_command() {
    let r = run("mkdir /d; touch /d/f; tar -cf /a.tar -C / d; file /a.tar");
    assert_eq!(r.stdout, "/a.tar: POSIX tar archive\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn file_json_and_xml_detection() {
    let r = run("printf '{\"a\":1}' > /f; file /f");
    assert_eq!(r.stdout, "/f: JSON text data\n");
    let r = run("printf '<?xml version=\"1.0\"?>' > /f; file /f");
    assert_eq!(r.stdout, "/f: XML document\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn file_unknown_extension_falls_back_to_ascii_text() {
    let r = run("printf 'just text\\n' > /f.weirdext; file /f.weirdext");
    assert_eq!(r.stdout, "/f.weirdext: ASCII text\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn file_binary_data() {
    let r = run("printf '\\x01\\x02\\x03' > /f; file /f");
    assert_eq!(r.stdout, "/f: data\n");
    assert_eq!(r.exit_code, 0);
}

// ── utils.rs: bc ────────────────────────────────────────────────────

#[test]
fn bc_math_library_flag_sets_scale() {
    // NOTE: pinned actual behavior — rust-bash's bc uses f64 arithmetic, so
    // 1/3 at scale=20 shows the f64 representation error; real bc prints
    // exactly 0.33333333333333333333.
    let r = run("echo '1/3' | bc -l");
    assert_eq!(r.stdout, "0.33333333333333331483\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn bc_reads_program_from_file() {
    let r = run("printf '1+2\\n' > /prog.bc; bc /prog.bc");
    assert_eq!(r.stdout, "3\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn bc_missing_file() {
    let r = run("bc /nope");
    // NOTE: pinned actual behavior — the VFS error message repeats the path.
    assert_eq!(r.stderr, "bc: /nope: No such file or directory: /nope\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn bc_quit_statement() {
    // NOTE: pinned actual behavior — real bc *exits* on `quit` (the 2+2
    // line would never run); rust-bash treats `quit` as a skipped line.
    let r = run("printf '1+1\\nquit\\n2+2\\n' | bc");
    assert_eq!(r.stdout, "2\n4\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn bc_invalid_scale_assignment() {
    let r = run("echo 'scale=abc' | bc");
    assert_eq!(r.stderr, "bc: parse error: scale=abc\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn bc_variable_assignment_and_use() {
    let r = run("printf 'x = 5\\nx * 2\\n' | bc");
    assert_eq!(r.stdout, "10\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn bc_assignment_with_invalid_rhs() {
    let r = run("echo 'y = 1 +' | bc");
    assert_eq!(r.stderr, "bc: parse error at position 3\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn bc_expression_error_reporting() {
    let r = run("echo '1 /' | bc");
    assert_eq!(r.stderr, "bc: parse error at position 3\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn bc_operator_coverage() {
    // Exercises every binary operator arm of bc_parse_expr, including the
    // precedence-break path (`2 * 3 + 1`). `%` goes through a %s format arg
    // so printf does not treat it as a format directive.
    let r = run("printf '%s\\n' '7 - 2' '5 % 3' '2 * 3 + 1' '2 ^ 3' | bc");
    assert_eq!(r.stdout, "5\n2\n7\n8\n");
    let r = run(
        "printf '1 == 1\\n1 == 2\\n1 != 2\\n1 != 1\\n1 < 2\\n2 < 1\\n2 > 1\\n1 > 2\\n1 <= 1\\n2 <= 1\\n1 >= 1\\n1 >= 2\\n' | bc",
    );
    assert_eq!(r.stdout, "1\n0\n1\n0\n1\n0\n1\n0\n1\n0\n1\n0\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn bc_division_and_modulo_by_zero() {
    let r = run("echo '1 / 0' | bc; echo '1 % 0' | bc");
    assert_eq!(r.stderr, "bc: divide by zero\nbc: divide by zero\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn bc_unary_operators() {
    let r = run("echo '-5 + 3' | bc; echo '+5' | bc");
    assert_eq!(r.stdout, "-2\n5\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn bc_parentheses() {
    let r = run("echo '(1 + 2) * 3' | bc");
    assert_eq!(r.stdout, "9\n");
    let r = run("echo '(1 + 2' | bc");
    assert_eq!(r.stderr, "bc: expected ')'\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn bc_variables_scale_and_undefined() {
    // NOTE: pinned actual behavior — with scale=2, rust-bash prints the
    // scale variable itself with two decimals; real bc prints `2`.
    let r = run("printf 'scale=2\\nscale\\n' | bc");
    assert_eq!(r.stdout, "2.00\n");
    // Undefined variables evaluate to 0 (real bc behavior).
    let r = run("echo 'qq' | bc");
    assert_eq!(r.stdout, "0\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn bc_unparseable_token() {
    let r = run("echo '@' | bc");
    assert_eq!(r.stderr, "bc: parse error at position 0\n");
    assert_eq!(r.exit_code, 1);
}
