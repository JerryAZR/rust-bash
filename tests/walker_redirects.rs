//! Behavioral tests for redirections, persistent FDs, and the `exec`
//! builtin in the walker, driven through the public `RustBash` API.

use rust_bash::RustBashBuilder;

fn shell() -> rust_bash::RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> (String, String, i32) {
    let mut sh = shell();
    let r = sh.exec(script).unwrap();
    (r.stdout, r.stderr, r.exit_code)
}

// ── exec: persistent redirects ──────────────────────────────────────

#[test]
fn exec_redirect_stdout_to_dev_null_and_back() {
    let (out, err, _) = run("exec > /dev/null; echo hidden; exec > /dev/stdout; echo shown");
    assert_eq!(out, "shown\n");
    assert_eq!(err, "");
}

#[test]
fn exec_redirect_stdout_to_dev_stderr_restores() {
    // exec > /dev/stderr clears the persistent fd, so output stays on stdout.
    let (out, _, _) = run("exec > /dev/stderr; echo where; echo done");
    assert_eq!(out, "where\ndone\n");
}

#[test]
fn exec_redirect_stderr_to_dev_null() {
    let (out, err, _) = run("exec 2> /dev/null; cat /missing; echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(err, "");
}

#[test]
fn exec_redirect_stdout_to_file_and_restore() {
    let (out, _, _) = run("exec > /f; echo one; exec > /dev/stdout; echo two; cat /f");
    assert_eq!(out, "two\none\n");
}

#[test]
fn exec_append_stdout_to_file() {
    let (out, _, _) = run("echo pre > /f; exec >> /f; echo more; exec > /dev/stdout; cat /f");
    assert_eq!(out, "pre\nmore\n");
}

#[test]
fn exec_append_to_dev_null_and_dev_stdout() {
    let (out, _, _) = run("exec >> /dev/null; echo x; exec >> /dev/stdout; echo y");
    assert_eq!(out, "y\n");
}

#[test]
fn exec_persistent_input_from_dev_null_then_close() {
    let (out, _, _) = run("exec 3< /dev/null; exec 3<&-; echo ok");
    assert_eq!(out, "ok\n");
}

#[test]
fn exec_persistent_input_file_and_read() {
    let (out, _, _) = run("echo abc > /f; exec 3< /f; read line <&3; echo $line");
    assert_eq!(out, "abc\n");
}

#[test]
fn exec_dup_output_from_persistent_fd() {
    let (out, _, _) = run("exec 3> /f; exec 4>&3; echo hi >&4; exec > /dev/stdout; cat /f");
    assert_eq!(out, "hi\n");
}

#[test]
fn exec_dup_output_from_standard_fd() {
    let (_, _, code) = run("exec 3>&1; echo rc=$?");
    assert_eq!(code, 0);
    let (_, _, code) = run("exec 2>&1; echo rc=$?");
    assert_eq!(code, 0);
}

#[test]
fn exec_move_fd_closes_source() {
    let (out, _, _) = run("exec 3> /f; exec 4>&3-; echo hi >&4; exec > /dev/stdout; cat /f");
    assert_eq!(out, "hi\n");
}

#[test]
fn exec_dup_to_unopen_fd_removes_mapping() {
    // fd 9 has no persistent mapping and is not a standard fd: the mapping
    // is removed; the outer redirect application then reports the bad fd.
    let (_, err, _) = run("exec 5>&9; echo rc=$?");
    assert_eq!(err, "rust-bash: 5: Bad file descriptor\n");
}

#[test]
fn exec_dup_input_from_persistent_fd() {
    let (out, _, _) = run("echo data > /f; exec 3< /f; exec 4<&3; read l <&4; echo $l");
    assert_eq!(out, "data\n");
}

#[test]
fn exec_output_and_error_to_file() {
    let (out, _, _) = run("exec &> /f; echo out; cat /missing; exec > /dev/stdout; cat /f");
    assert_eq!(
        out,
        "out\ncat: /missing: No such file or directory: /missing\n"
    );
}

#[test]
fn exec_output_and_error_to_dev_null() {
    let (out, _, _) = run("exec &> /dev/null; echo quiet; exec > /dev/stdout; echo loud");
    assert_eq!(out, "loud\n");
}

#[test]
fn exec_redirect_empty_filename_errors_twice() {
    // Pinned actual behavior: the failure is reported once by the exec
    // builtin and once by the outer redirect application.
    let (out, err, _) = run("e=; exec > $e; echo after");
    assert_eq!(out, "after\n");
    assert_eq!(
        err,
        "rust-bash: : No such file or directory\nrust-bash: : No such file or directory\n"
    );
}

// ── exec: {varname} FD allocation ───────────────────────────────────

#[test]
fn exec_fd_variable_alloc_write() {
    let (out, _, _) = run("exec {fd}> /f; echo via_fd >&$fd; exec > /dev/stdout; cat /f");
    assert_eq!(out, "via_fd\n");
}

#[test]
fn exec_fd_variable_alloc_dev_null() {
    let (out, _, _) = run("exec {fd}> /dev/null; echo x; exec > /dev/stdout; echo y; echo fd=$fd");
    assert_eq!(out, "x\ny\nfd=10\n");
}

#[test]
fn exec_fd_variable_alloc_dev_stdout() {
    // Pinned actual behavior (suspected divergence vs bash, which would
    // print "hello"): the /dev/stdout target removes the new mapping, so
    // writing to the allocated fd fails.
    let (_, err, code) = run("exec {fd}> /dev/stdout; echo hello >&$fd");
    assert_eq!(err, "rust-bash: 1: Bad file descriptor\n");
    assert_eq!(code, 1);
}

#[test]
fn exec_fd_variable_alloc_append() {
    let (out, _, _) =
        run("echo pre > /f; exec {fd}>> /f; echo ap >&$fd; exec > /dev/stdout; cat /f");
    assert_eq!(out, "pre\nap\n");
}

#[test]
fn exec_fd_variable_alloc_append_dev_paths() {
    let (out, _, _) = run("exec {fd}>> /dev/null; echo x; exec {fd2}>> /dev/stdout; echo y");
    assert_eq!(out, "x\ny\n");
}

#[test]
fn exec_fd_variable_alloc_read() {
    let (out, _, _) = run("echo zzz > /f; exec {fd}< /f; read l <&$fd; echo $l");
    assert_eq!(out, "zzz\n");
    let (out, _, _) = run("exec {fd}< /dev/null; echo ok");
    assert_eq!(out, "ok\n");
}

#[test]
fn exec_fd_variable_alloc_readwrite() {
    let (out, _, _) =
        run("echo '' > /rw; exec {fd}<>/rw; echo zz >&$fd; exec > /dev/stdout; cat /rw");
    assert_eq!(out, "zz\n");
    let (out, _, _) = run("exec {fd}<> /dev/null; echo ok");
    assert_eq!(out, "ok\n");
}

#[test]
fn exec_fd_variable_alloc_readwrite_missing_file_divergence() {
    // Pinned actual behavior (suspected divergence vs bash, which creates
    // the file): input collection fails before the exec builtin runs.
    let (_, err, _) = run("exec {fd}<>/rw; echo zz >&$fd");
    assert_eq!(
        err,
        "rust-bash: /rw: No such file or directory\nrust-bash: : ambiguous redirect\n"
    );
}

#[test]
fn exec_fd_variable_alloc_dup_output_is_ignored() {
    // `exec {fd}>&2` allocates the fd number but does not map it (the dup
    // kind is not handled in the alloc loop), so writing to it fails.
    let (_, err, code) = run("exec {fd}>&2; echo via >&$fd");
    assert_eq!(err, "rust-bash: 1: Bad file descriptor\n");
    assert_eq!(code, 1);
}

#[test]
fn exec_fd_variable_alloc_too_many_arguments() {
    let (_, err, _) = run("exec {fd}> /f extra; echo rc=$?");
    assert_eq!(err, "rust-bash: exec: too many arguments\n");
}

#[test]
fn exec_fd_variable_close() {
    let (_, _, code) = run("exec {fd}> /f; exec {fd}>&-; echo rc=$?");
    assert_eq!(code, 0);
}

#[test]
fn exec_fd_variable_invalid_name_is_command() {
    // `{9}` is not a valid identifier, so it is treated as a command name.
    let (_, err, code) = run("exec {9}> /f; echo rc=$?");
    assert_eq!(err, "{9}: command not found\n");
    assert_eq!(code, 127);
}

#[test]
fn exec_redirect_filename_arithmetic_error() {
    let (out, err, _) = run("exec > /f$(( 1/0 )); echo after");
    assert_eq!(out, "after\n");
    assert_eq!(err, "rust-bash: line 1: arithmetic: division by zero\n");
}

#[test]
fn exec_close_output_fd() {
    let (out, _, _) = run("exec 3> /f; exec 3>&-; echo after");
    assert_eq!(out, "after\n");
}

#[test]
fn exec_dup_output_to_stdin_fd() {
    let (_, _, code) = run("exec 3>&0; echo rc=$?");
    assert_eq!(code, 0);
}

#[test]
fn exec_with_here_inputs_is_noop() {
    let (_, _, code) = run("exec <<< hi; echo rc=$?");
    assert_eq!(code, 0);
    let (_, _, code) = run("exec <<EOF\nbody\nEOF\necho rc=$?");
    assert_eq!(code, 0);
}

#[test]
fn exec_fd_variable_alloc_with_output_and_error_redirect() {
    // &> is not a File redirect, so the alloc loop skips it but still
    // allocates the fd number.
    let (out, _, _) = run("exec {fd}&> /f; echo rc=$? fd=$fd");
    assert_eq!(out, "rc=0 fd=10\n");
}

#[test]
fn exec_fd_variable_alloc_with_extra_file_redirect() {
    // Only the first File redirect is applied to the allocated fd.
    let (out, _, _) = run("exec {fd}> /f 2>&1; echo rc=$? fd=$fd");
    assert_eq!(out, "rc=0 fd=10\n");
}

// ── exec: command mode ──────────────────────────────────────────────

#[test]
fn exec_command_replaces_shell() {
    let (out, _, _) = run("exec echo hi; echo after");
    assert_eq!(out, "hi\n");
}

#[test]
fn exec_command_with_input_redirect() {
    let (out, _, _) = run("echo data > /f; exec cat < /f; echo after");
    assert_eq!(out, "data\n");
}

#[test]
fn exec_command_with_missing_input_file() {
    let (out, err, _) = run("exec echo hi < /missing; echo after");
    assert_eq!(out, "after\n");
    assert_eq!(err, "rust-bash: /missing: No such file or directory\n");
}

// ── Input redirection ───────────────────────────────────────────────

#[test]
fn input_dup_from_resolved_fd() {
    let (out, _, _) = run("echo content > /f; cat 3< /f 0<&3");
    assert_eq!(out, "content\n");
}

#[test]
fn input_from_dev_stdin_uses_pipe_data() {
    let (out, _, _) = run("printf 'stdin_data' | cat < /dev/stdin");
    assert_eq!(out, "stdin_data");
}

#[test]
fn input_dup_from_persistent_dev_null() {
    let (out, _, _) = run("exec 3< /dev/null; cat <&3; echo rc=$?");
    assert_eq!(out, "rc=0\n");
}

#[test]
fn input_dup_from_persistent_output_fd_falls_through() {
    // Duplicating stdin from an output-only fd yields no input redirect,
    // so the pipe data is used instead. Pinned actual behavior.
    let (out, _, _) = run("exec 3> /f; echo x | cat <&3; echo rc=$?");
    assert_eq!(out, "x\nrc=0\n");
}

#[test]
fn herestring_on_nonzero_fd_is_ignored_for_stdin() {
    let (out, _, _) = run("cat 3<<< ignored <<< real");
    assert_eq!(out, "real\n");
}

#[test]
fn heredoc_on_nonzero_fd_is_ignored_for_stdin() {
    let (out, _, _) = run("cat 3<<EOF <<EOF2\nthree\nEOF\nreal\nEOF2");
    assert_eq!(out, "real\n");
}

// ── Heredoc body expansion ──────────────────────────────────────────

#[test]
fn heredoc_escape_sequences() {
    let (out, _, _) = run(
        "cat <<EOF\nline1\\\ncont\nback\\\\slash\ndollar\\$var\nbacktick\\`x\\`\nquote\"q\"\nother\\z\nEOF",
    );
    assert_eq!(
        out,
        "line1cont\nback\\slash\ndollar$var\nbacktick`x`\nquote\"q\"\nother\\z\n"
    );
}

#[test]
fn heredoc_quoted_delimiter_disables_expansion() {
    let (out, _, _) = run("cat <<'EOF'\nno $expansion \\\nEOF");
    assert_eq!(out, "no $expansion \\\n");
}

#[test]
fn heredoc_dash_strips_tabs() {
    let (out, _, _) = run("cat <<-EOF\n\t\ttabbed\n\tEOF");
    assert_eq!(out, "tabbed\n");
}

#[test]
fn heredoc_nounset_expansion_error() {
    let (_, err, code) = run("set -u; cat <<EOF\n$undef\nEOF");
    assert_eq!(err, "rust-bash: undef: unbound variable\n");
    assert_eq!(code, 1);
}

// ── mapfile with directory / missing input ──────────────────────────

#[test]
fn mapfile_reading_directory_uses_empty_input() {
    let (out, err, _) = run("mkdir -p /d; mapfile < /d; echo rc=$?");
    assert_eq!(out, "rc=0\n");
    assert_eq!(err, "");
}

#[test]
fn mapfile_missing_input_file_errors() {
    let (_, err, _) = run("mapfile < /missing > /out; echo rc=$?");
    assert_eq!(err, "rust-bash: /missing: No such file or directory\n");
}

// ── &> redirects ────────────────────────────────────────────────────

#[test]
fn output_and_error_empty_filename() {
    let (out, err, _) = run("e=; echo x &> $e; echo after");
    assert_eq!(out, "x\nafter\n");
    assert_eq!(err, "rust-bash: : No such file or directory\n");
}

#[test]
fn noclobber_blocks_output_and_error_on_existing_file() {
    let (_, err, _) = run("set -C; echo a > /f; echo b &> /f; echo rc=$?");
    assert_eq!(err, "rust-bash: /f: cannot overwrite existing file\n");
}

#[test]
fn noclobber_allows_append_output_and_error() {
    let (out, _, _) = run("set -C; echo a > /f; echo b &>> /f; cat /f");
    assert_eq!(out, "a\nb\n");
}

// ── Persistent fd fallback ──────────────────────────────────────────

#[test]
fn persistent_stderr_file_captures_errors() {
    let (out, _, _) = run("exec 2> /err; cat /missing; exec 2> /dev/stderr; cat /err");
    assert_eq!(out, "cat: /missing: No such file or directory: /missing\n");
}

#[test]
fn persistent_stdout_file_appends_across_commands() {
    let (out, _, _) = run("exec > /f; echo line1; echo line2; exec > /dev/stdout; cat /f");
    assert_eq!(out, "line1\nline2\n");
}

#[test]
fn persistent_input_fd_on_output_streams_is_ignored() {
    // An InputFile mapping on fd 1 / fd 2 matches no fallback arm.
    let (out, _, _) = run("echo d > /f; exec 1< /f; echo x; echo rc=$?");
    assert_eq!(out, "x\nrc=0\n");
    let (out, err, _) = run("echo d > /f; exec 2< /f; cat /missing; echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(err, "cat: /missing: No such file or directory: /missing\n");
}

// ── Per-command redirect edge cases ─────────────────────────────────

#[test]
fn ambiguous_redirect_target() {
    let (out, err, _) = run("v='a b'; echo x >& $v; echo after");
    assert_eq!(out, "after\n");
    assert_eq!(err, "rust-bash: a b: ambiguous redirect\n");
}

#[test]
fn readwrite_redirect_missing_file_divergence() {
    // Pinned actual behavior (suspected divergence vs bash, which creates
    // the file and writes x): input collection fails before dispatch; the
    // file is created empty during error handling.
    let (out, err, _) = run("echo x 1<>/rwf; cat /rwf");
    assert_eq!(out, "");
    assert_eq!(err, "rust-bash: /rwf: No such file or directory\n");
}

#[test]
fn readwrite_redirect_stderr_missing_file() {
    // Pinned actual behavior: the shell's redirect error message itself is
    // written into the target file, so it surfaces when the file is read.
    let (out, err, code) = run("cat /missing 2<>/rwf2; cat /rwf2");
    assert_eq!(out, "rust-bash: /rwf2: No such file or directory\n");
    assert_eq!(err, "");
    assert_eq!(code, 0);
}

#[test]
fn redirect_stderr_to_dev_stdout() {
    let (out, err, _) = run("cat /missing 2> /dev/stdout");
    assert_eq!(out, "cat: /missing: No such file or directory: /missing\n");
    assert_eq!(err, "");
}

#[test]
fn redirect_stdout_to_dev_stderr() {
    let (out, err, _) = run("echo x > /dev/stderr");
    assert_eq!(out, "");
    assert_eq!(err, "x\n");
}

#[test]
fn redirect_to_dev_full_fails_with_write_error() {
    let (_, err, _) = run("echo x > /dev/full; echo rc=$?");
    assert_eq!(
        err,
        "rust-bash: write error: /dev/full: No space left on device\n"
    );
    let (_, err, _) = run("cat /missing 2> /dev/full; echo rc=$?");
    assert_eq!(
        err,
        "rust-bash: write error: /dev/full: No space left on device\n"
    );
}

#[test]
fn close_stderr() {
    let (out, err, _) = run("cat /missing 2>&-; echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(err, "");
}

#[test]
fn move_fd_from_stderr() {
    // `3>&2-` duplicates fd 2 to fd 3 and closes fd 2.
    let (out, _, _) = run("echo x 3>&2-");
    assert_eq!(out, "x\n");
}

#[test]
fn dup_to_bad_fd() {
    let (out, err, _) = run("echo x 1>&9; echo after");
    assert_eq!(out, "after\n");
    assert_eq!(err, "rust-bash: 1: Bad file descriptor\n");
}

#[test]
fn dup_word_to_dev_full_reports_write_error() {
    let (out, err, _) = run("echo x 2>& /dev/full; echo rc=$?");
    assert_eq!(out, "x\nrc=1\n");
    assert_eq!(
        err,
        "rust-bash: write error: /dev/full: No space left on device\n"
    );
}

#[test]
fn bare_dup_word_to_dev_stdout_writes_vfs_file() {
    // Pinned actual behavior (suspected divergence vs bash, which prints
    // the output): `>& /dev/stdout` treats the target as a plain filename
    // and writes a VFS file literally named /dev/stdout.
    let (out, _, _) = run("echo x >& /dev/stdout; cat /dev/stdout");
    assert_eq!(out, "x\n");
}

#[test]
fn dup_input_to_bad_fd_nonzero() {
    let (out, err, _) = run("echo x 3<&9; echo after");
    assert_eq!(out, "x\nafter\n");
    assert_eq!(err, "rust-bash: 3: Bad file descriptor\n");
}

#[test]
fn close_stdout() {
    let (out, _, _) = run("echo x >&-; echo after");
    assert_eq!(out, "after\n");
}

#[test]
fn dup_stdout_to_stdin_fd_is_noop() {
    let (out, _, _) = run("echo x 1>&0; echo rc=$?");
    assert_eq!(out, "x\nrc=0\n");
}

#[test]
fn bare_dup_word_to_dev_null_discards() {
    let (out, _, _) = run("echo x >& /dev/null; echo after");
    assert_eq!(out, "after\n");
}

#[test]
fn exec_with_pipe_extension_redirect() {
    // `|&` synthesizes a `2>&1` redirect with an Fd target.
    let (_, _, code) = run("exec |& cat; echo rc=$?");
    assert_eq!(code, 0);
}

// ── Duplicating into persistent fds ─────────────────────────────────

#[test]
fn dup_stderr_to_persistent_output_file() {
    let (out, _, _) = run("exec 3> /f; echo x 2>&3; exec > /dev/stdout; cat /f");
    assert_eq!(out, "x\n");
}

#[test]
fn dup_stderr_to_persistent_readwrite_file() {
    let (out, _, _) =
        run("echo '' > /f; exec 3<>/f; cat /missing 2>&3; exec > /dev/stdout; cat /f");
    assert_eq!(out, "cat: /missing: No such file or directory: /missing\n");
}

#[test]
fn dup_stderr_to_persistent_dev_null() {
    let (out, _, _) = run("exec 3> /dev/null; cat /missing 2>&3; echo ok");
    assert_eq!(out, "ok\n");
}

#[test]
fn dup_stderr_to_persistent_dup_of_stdout() {
    let (out, err, _) = run("exec 3>&1; cat /missing 2>&3");
    assert_eq!(out, "cat: /missing: No such file or directory: /missing\n");
    assert_eq!(err, "");
}

#[test]
fn dup_stdout_to_persistent_dup_of_stderr() {
    let (out, err, _) = run("exec 3>&2; echo v 1>&3");
    assert_eq!(out, "");
    assert_eq!(err, "v\n");
}

#[test]
fn dup_stdout_to_persistent_input_file_is_noop() {
    // Writing to an input-only fd mapping is silently ignored.
    // Pinned actual behavior.
    let (out, _, _) = run("echo d > /f; exec 3< /f; echo u 1>&3; echo rc=$?");
    assert_eq!(out, "u\nrc=0\n");
}

#[test]
fn persistent_readwrite_offset_pads_after_external_truncate() {
    // The persistent offset outlives an external truncation; the gap is
    // zero-padded on the next write.
    let (out, _, _) = run(
        "echo '' > /f; exec 3<>/f; echo abc >&3; echo 'x' > /f; echo xy >&3; \
         exec > /dev/stdout; od -c /f",
    );
    assert_eq!(out, "0000000   x  \\n  \\0  \\0   x   y  \\n\n0000007\n");
}

// ── Process substitution ────────────────────────────────────────────

#[test]
fn process_substitution_write_as_argument() {
    let (out, _, _) = run("echo hi >(cat)");
    assert_eq!(out, "hi /tmp/.proc_sub_0\n");
}

#[test]
fn process_substitution_read_as_argument() {
    let (out, _, _) = run("cat <(printf abc)");
    assert_eq!(out, "abc");
}

#[test]
fn process_substitution_write_as_redirect_target() {
    let (out, _, _) = run("echo hi > >(cat)");
    assert_eq!(out, "hi\n");
}

#[test]
fn process_substitution_creates_tmp_when_missing() {
    let (out, _, _) = run("rm -rf /tmp; cat <(printf xyz)");
    assert_eq!(out, "xyz");
}

#[test]
fn brace_group_with_stdin_dup_from_persistent_fd() {
    let (out, _, _) = run("echo data > /f; exec 3< /f; { cat; } 0<&3");
    assert_eq!(out, "data\n");
}

#[test]
fn brace_group_with_output_and_error_redirect() {
    let (out, _, _) = run("{ echo out; cat /missing; } &> /f; cat /f");
    assert_eq!(
        out,
        "out\ncat: /missing: No such file or directory: /missing\n"
    );
}

#[test]
fn brace_group_with_output_and_error_append() {
    let (out, _, _) = run("echo old > /f; { echo new; } &>> /f; cat /f");
    assert_eq!(out, "old\nnew\n");
}

// ── Binary pipeline data ────────────────────────────────────────────

#[test]
fn binary_data_flows_through_pipeline() {
    let (out, _, _) = run("printf 'a\\0b' | wc -c");
    assert_eq!(out, "3\n");
}

#[test]
fn std_fd_move_close_clears_that_stream() {
    // `2>&1-` moves fd 2 onto fd 1's target, then closes fd 1: the echoed
    // line is discarded with the closed stdout stream. (bash errors on the
    // write instead; divergence pinned, behavior intentionally quiet here.)
    let (out, err, code) = run("echo hi 2>&1-; echo after");
    assert_eq!(out, "after\n");
    assert_eq!(err, "");
    assert_eq!(code, 0);
}

#[test]
fn process_substitution_limit_error_propagates() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};
    let limits = ExecutionLimits {
        max_string_length: 10,
        ..ExecutionLimits::default()
    };
    let mut sh = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap();
    let err = sh.exec("cat <(x=aaaaaaaaaaaaaaaaaaaa)").unwrap_err();
    assert!(
        matches!(err, rust_bash::RustBashError::LimitExceeded { .. }),
        "unexpected error: {err:?}"
    );
}

#[test]
fn exec_command_limit_error_propagates() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};
    let limits = ExecutionLimits {
        max_command_count: 3,
        ..ExecutionLimits::default()
    };
    let mut sh = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap();
    let err = sh
        .exec("exec sh -c 'echo 1; echo 2; echo 3; echo 4; echo 5'")
        .unwrap_err();
    assert!(
        matches!(err, rust_bash::RustBashError::LimitExceeded { .. }),
        "unexpected error: {err:?}"
    );
}
