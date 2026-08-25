//! Coverage-triage tests for `src/commands/exec_cmds.rs` (xargs, find).
//!
//! Every test here exists to cover a previously-uncovered region of that
//! file: untested flags, expression-parser error paths, and exec-callback
//! edge cases. Where behavior diverges from real bash/GNU, the actual
//! behavior is pinned with a comment (runtime behavior is intentionally
//! not changed).

use std::collections::HashMap;

use rust_bash::{ExecResult, RustBash, RustBashBuilder};

fn shell() -> RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> ExecResult {
    shell().exec(script).unwrap()
}

fn run_with_files(files: &[(&str, &str)], script: &str) -> ExecResult {
    let map: HashMap<String, Vec<u8>> = files
        .iter()
        .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
        .collect();
    RustBashBuilder::new()
        .files(map)
        .build()
        .unwrap()
        .exec(script)
        .unwrap()
}

// ── xargs ────────────────────────────────────────────────────────────

#[test]
fn xargs_double_dash_ends_option_parsing() {
    let r = run("printf 'a b\n' | xargs -- echo");
    assert_eq!(r.stdout, "a b\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn xargs_missing_option_arguments() {
    let r = run("echo x | xargs -I");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "xargs: option requires an argument -- 'I'\n");
    assert_eq!(r.exit_code, 1);

    let r = run("echo x | xargs -n");
    assert_eq!(r.stderr, "xargs: option requires an argument -- 'n'\n");
    assert_eq!(r.exit_code, 1);

    let r = run("echo x | xargs -d");
    assert_eq!(r.stderr, "xargs: option requires an argument -- 'd'\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn xargs_invalid_max_args_values() {
    let r = run("echo x | xargs -n 0");
    assert_eq!(r.stderr, "xargs: invalid number for -n: '0'\n");
    assert_eq!(r.exit_code, 1);

    let r = run("echo x | xargs -n abc");
    assert_eq!(r.stderr, "xargs: invalid number for -n: 'abc'\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn xargs_unknown_option_becomes_command_name() {
    // PINNED DIVERGENCE: GNU xargs rejects unknown options
    // ("xargs: invalid option -- 'Z'"); rust-bash treats the unknown option
    // as the start of the command to run, which then fails to resolve.
    let r = run("printf 'x\n' | xargs -Z");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "-Z: command not found\n");
    assert_eq!(r.exit_code, 127);
}

#[test]
fn xargs_delimiter_escape_sequences() {
    // -d takes a literal backslash-escape string: '\n', '\t', '\0'.
    let r = run("printf 'x\ny\n' | xargs -d '\\n' echo");
    assert_eq!(r.stdout, "x y\n");
    assert_eq!(r.exit_code, 0);

    let r = run("printf 'x\ty' | xargs -d '\\t' echo");
    assert_eq!(r.stdout, "x y\n");
    assert_eq!(r.exit_code, 0);

    // printf emits the NUL bytes; a piped stdin reaches xargs unmodified.
    let r = run("printf 'x\0y\0' | xargs -d '\\0' echo");
    assert_eq!(r.stdout, "x y\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn xargs_replace_mode_with_empty_input_runs_nothing() {
    let r = run("printf '' | xargs -I {} echo got-{}");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn xargs_exec_error_when_command_fails_to_parse() {
    // The exec callback returns Err when the assembled command line does not
    // parse ("echo a&&"). Covers the Err arm of the no-input path and all
    // three batching modes.
    // NOTE: relies on shell_join not quoting the token (no whitespace or
    // quotes in "a&&"), so "echo a&&" fails to parse in the exec callback.
    let r = run("printf '' | xargs 'a&&'");
    assert_eq!(r.stdout, "");
    assert!(r.stderr.starts_with("xargs: parse error: "));
    assert_eq!(r.exit_code, 1);

    let r = run("printf 'a&&\n' | xargs -I {} echo {}");
    assert!(r.stderr.starts_with("xargs: parse error: "));
    assert_eq!(r.exit_code, 1);

    let r = run("printf 'a&&\n' | xargs -n 1 echo");
    assert!(r.stderr.starts_with("xargs: parse error: "));
    assert_eq!(r.exit_code, 1);

    let r = run("printf 'a&&\n' | xargs echo");
    assert!(r.stderr.starts_with("xargs: parse error: "));
    assert_eq!(r.exit_code, 1);
}

// ── find: expression parser errors ───────────────────────────────────

#[test]
fn find_missing_option_arguments() {
    let r = run("find / -maxdepth");
    assert_eq!(r.stderr, "find: missing argument to '-maxdepth'\n");
    assert_eq!(r.exit_code, 1);

    let r = run("find / -mindepth");
    assert_eq!(r.stderr, "find: missing argument to '-mindepth'\n");
    assert_eq!(r.exit_code, 1);

    let r = run("find / -name");
    assert_eq!(r.stderr, "find: missing argument to '-name'\n");
    assert_eq!(r.exit_code, 1);

    let r = run("find / -type");
    assert_eq!(r.stderr, "find: missing argument to '-type'\n");
    assert_eq!(r.exit_code, 1);

    let r = run("find / -newer");
    assert_eq!(r.stderr, "find: missing argument to '-newer'\n");
    assert_eq!(r.exit_code, 1);

    let r = run("find / -exec echo");
    assert_eq!(r.stderr, "find: missing argument to '-exec'\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn find_unknown_predicate_fails() {
    let r = run("find / -bogus");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "find: unknown predicate '-bogus'\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn find_negation_without_expression_fails() {
    let r = run("find / -not");
    assert_eq!(r.stderr, "find: expected expression\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn find_parenthesized_expression() {
    let r = run_with_files(
        &[("/t/a.txt", "1\n"), ("/t/b.md", "2\n")],
        "find /t '(' -name '*.txt' -o -name '*.md' ')' -type f",
    );
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "/t/a.txt\n/t/b.md\n");
}

#[test]
fn find_unclosed_parenthesis_fails() {
    let r = run("find / '(' -name x");
    assert_eq!(r.stderr, "find: missing closing ')'\n");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn find_extra_closing_parenthesis_fails() {
    let r = run("find / '(' -name x ')' ')'");
    assert_eq!(r.stderr, "find: unexpected argument ')'\n");
    assert_eq!(r.exit_code, 1);
}

// ── find: predicates ─────────────────────────────────────────────────

#[test]
fn find_type_symlink() {
    let r = run_with_files(
        &[("/t/a.txt", "1\n")],
        "ln -s /t/a.txt /t/link && find /t -type l",
    );
    assert_eq!(r.stdout, "/t/link\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn find_type_unknown_matches_nothing() {
    let r = run_with_files(&[("/t/a.txt", "1\n")], "find /t -type x");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn find_stat_failures_on_dangling_symlink_are_skipped() {
    // A dangling symlink makes stat() fail inside the walk: the plain walk
    // still prints it (default action runs before the stat guard), while
    // -type f and -empty silently treat it as non-matching.
    let setup = "mkdir /t && ln -s /gone /t/dangling";

    let r = run(&format!("{setup} && find /t"));
    assert_eq!(r.stdout, "/t\n/t/dangling\n");
    assert_eq!(r.exit_code, 0);

    let r = run(&format!("{setup} && find /t -type f"));
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);

    let r = run(&format!("{setup} && find /t -empty"));
    assert_eq!(r.stdout, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn find_newer_matches_files_with_later_mtime() {
    // ref is created first, then /t/new after a sleep so its mtime is
    // strictly later regardless of clock resolution.
    let r = run(
        "mkdir /t && echo old > /t/ref && sleep 0.1 && echo new > /t/new && find /t -newer /t/ref -type f",
    );
    assert_eq!(r.stdout, "/t/new\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn find_newer_with_missing_reference_matches_nothing() {
    let r = run_with_files(&[("/t/a", "1\n")], "find /t -newer /t/nonexistent");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn find_newer_skips_paths_whose_stat_fails() {
    let r = run("mkdir /t && ln -s /gone /t/dangling && echo x > /t/ref && find /t -newer /t/ref");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn find_explicit_print_action() {
    let r = run_with_files(
        &[("/t/a.txt", "1\n"), ("/t/b.md", "2\n")],
        "find /t -name '*.txt' -print",
    );
    assert_eq!(r.stdout, "/t/a.txt\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

// ── find: -exec ──────────────────────────────────────────────────────

#[test]
fn find_exec_each_with_quoted_placeholder_path() {
    // A filename containing a space exercises shell_escape's quoting arm.
    let r = run_with_files(
        &[("/t/a b.txt", "1\n")],
        "find /t -type f -exec echo {} ';'",
    );
    assert_eq!(r.stdout, "/t/a b.txt\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn find_exec_error_when_command_fails_to_parse() {
    // NOTE: relies on shell_escape not quoting '&', so the reassembled
    // command line "echo a&&" fails to parse inside the exec callback.
    let r = run_with_files(&[("/t/a", "1\n")], "find /t -type f -exec echo 'a&&' ';'");
    assert_eq!(r.stdout, "");
    assert!(r.stderr.starts_with("find: exec error: parse error: "));
    assert_eq!(r.exit_code, 1);
}

#[test]
fn find_exec_batch_with_placeholder() {
    let r = run_with_files(
        &[("/t/a.txt", "1\n"), ("/t/b.txt", "2\n")],
        "find /t -name '*.txt' -exec echo got: {} +",
    );
    assert_eq!(r.stdout, "got: /t/a.txt /t/b.txt\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn find_exec_batch_without_placeholder_appends_paths() {
    let r = run_with_files(
        &[("/t/a.txt", "1\n"), ("/t/b.txt", "2\n")],
        "find /t -name '*.txt' -exec echo pre +",
    );
    assert_eq!(r.stdout, "pre /t/a.txt /t/b.txt\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn find_exec_batch_propagates_nonzero_exit() {
    let r = run_with_files(&[("/t/a", "1\n")], "find /t -type f -exec false +");
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 1);
}

#[test]
fn find_exec_batch_exec_error_when_command_fails_to_parse() {
    let r = run_with_files(&[("/t/a", "1\n")], "find /t -type f -exec echo '$(x' +");
    assert_eq!(r.stdout, "");
    assert!(r.stderr.starts_with("find: exec error: parse error: "));
    assert_eq!(r.exit_code, 1);
}

#[test]
fn find_exec_batch_inside_and_or_not_expressions() {
    // collect_batch_cmds recurses through And/Or/Not to find ExecBatch nodes.
    let r = run_with_files(
        &[("/t/a.txt", "1\n"), ("/t/b.md", "2\n")],
        "find /t -type f -a -name '*.txt' -exec echo and: {} +",
    );
    assert_eq!(r.stdout, "and: /t/a.txt\n");
    assert_eq!(r.exit_code, 0);

    // Or: the .md side matches the Name primary (no action runs for it);
    // every non-matching path — including the /t start directory itself —
    // falls through to the batch (same as GNU find's implicit -o grouping).
    let r = run_with_files(
        &[("/t/a.txt", "1\n"), ("/t/b.md", "2\n")],
        "find /t -name '*.md' -o -exec echo or: {} +",
    );
    assert_eq!(r.stdout, "or: /t /t/a.txt\n");
    assert_eq!(r.exit_code, 0);

    // Not: ExecBatch still collects every visited path, and the batch runs.
    let r = run_with_files(
        &[("/t/a", "1\n")],
        "find /t -type f -not -exec echo not: {} +",
    );
    assert_eq!(r.stdout, "not: /t/a\n");
    assert_eq!(r.exit_code, 0);
}
