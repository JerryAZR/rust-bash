//! Behavioral tests for xtrace (`set -x`), verbose mode (`set -v`), and
//! trace rendering helpers in the walker, driven through the public API.

use rust_bash::RustBashBuilder;

fn shell() -> rust_bash::RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> (String, String, i32) {
    let mut sh = shell();
    let r = sh.exec(script).unwrap();
    (r.stdout, r.stderr, r.exit_code)
}

// ── PS4 handling ────────────────────────────────────────────────────

#[test]
fn empty_ps4_uses_default_prefix() {
    let (out, err, _) = run("set -x; PS4=; echo hi");
    assert_eq!(out, "hi\n");
    assert_eq!(err, "+ PS4=\n+ echo hi\n");
}

#[test]
fn custom_ps4_is_expanded() {
    let (_, err, _) = run("PS4='>> '; set -x; echo hi");
    assert_eq!(err, ">> echo hi\n");
}

// ── xtrace word quoting ─────────────────────────────────────────────

#[test]
fn xtrace_quotes_empty_word() {
    let (_, err, _) = run("set -x; echo ''");
    assert_eq!(err, "+ echo ''\n");
}

#[test]
fn xtrace_ansi_c_quotes_control_characters() {
    let (out, err, _) = run(r"set -x; echo $'\a\b\t\n\v\f\r\E\001'");
    assert_eq!(out, "\u{7}\u{8}\t\n\u{b}\u{c}\r\u{1b}\u{1}\n");
    assert_eq!(err, "+ echo $'\\a\\b\\t\\n\\v\\f\\r\\E\\001'\n");
}

#[test]
fn xtrace_quotes_single_quote_in_plain_word() {
    let (_, err, _) = run(r"set -x; echo $'q\'w'");
    assert_eq!(err, "+ echo q\\'w\n");
}

#[test]
fn xtrace_ansi_c_quotes_single_quote_with_control_char() {
    let (_, err, _) = run(r"set -x; echo $'q\'w\a'");
    assert_eq!(err, "+ echo $'q\\'w\\a'\n");
}

#[test]
fn xtrace_ansi_c_quotes_backslash_with_control_char() {
    let (_, err, _) = run(r"set -x; echo $'\\\a'");
    assert_eq!(err, "+ echo $'\\\\\\a'\n");
}

#[test]
fn xtrace_plain_word_with_tab_is_not_ansi_c_quoted() {
    // A lone tab does not trigger $'...' quoting.
    let (_, err, _) = run(r"set -x; echo $'q\'w\t'");
    assert_eq!(err, "+ echo q\\'w\t\n");
    let (_, err, _) = run(r"set -x; echo $'\\\t'");
    assert_eq!(err, "+ echo '\\\t'\n");
}

#[test]
fn xtrace_ansi_c_quotes_raw_bytes_as_octal() {
    // Byte 0xff is carried as an internal marker char and rendered octal.
    let (_, err, _) = run(r"set -x; echo $'\xff'");
    assert_eq!(err, "+ echo $'\\377'\n");
}

// ── Verbose mode ────────────────────────────────────────────────────

#[test]
fn verbose_echoes_source_lines() {
    let (out, err, _) = run("set -v; echo hi");
    assert_eq!(out, "hi\n");
    assert_eq!(err, "set -v; echo hi\n");
}

#[test]
fn verbose_skips_lines_beyond_source_via_eval() {
    // The eval'd commands have AST line numbers relative to the eval string,
    // which exceed the outer source's line count; no snippet is printed.
    let (out, err, _) = run("set -v; eval 'echo x\necho y'");
    assert_eq!(out, "x\ny\n");
    assert_eq!(err, "set -v; eval 'echo x\necho y'\n");
}

#[test]
fn verbose_skips_trap_body_lines_beyond_source() {
    // The EXIT trap body has AST line numbers relative to the trap string;
    // line 2 exceeds the one-line outer source, so no snippet is printed.
    let (out, err, _) = run("set -v; trap 'echo x\necho y' EXIT");
    assert_eq!(out, "x\ny\n");
    assert_eq!(err, "set -v; trap 'echo x\necho y' EXIT\n");
}

// ── xtrace rendering of array assignments ───────────────────────────

#[test]
fn xtrace_declare_array_with_extra_whitespace() {
    let (_, err, _) = run("set -x; declare -a arr=(  a   b )");
    assert_eq!(err, "+ arr=('a' 'b')\n+ declare -a arr\n");
}

#[test]
fn xtrace_declare_array_values_needing_quotes() {
    let (_, err, _) = run("set -x; declare -a 'a=(x*y z)'");
    assert_eq!(err, "+ a=('x*y' 'z')\n+ declare -a a\n");
}

#[test]
fn xtrace_readonly_array_assignment() {
    let (_, err, _) = run("set -x; readonly -a r=(1 2)");
    assert_eq!(err, "+ readonly -a 'r=(1 2)'\n+ r=(1 2)\n");
}

#[test]
fn xtrace_declare_with_invalid_array_name_falls_back() {
    // `my-map` is not a valid array name for trace rendering, so the
    // assignment arg is traced as a plain argument.
    let (_, err, code) = run("set -x; declare 'my-map=(x)'");
    assert_eq!(
        err,
        "+ declare 'my-map=(x)'\nrust-bash: declare: `my-map=(x)': not a valid identifier\n"
    );
    assert_eq!(code, 1);
}

#[test]
fn xtrace_indexed_array_assignment() {
    let (_, err, _) = run("set -x; a=(1 2)");
    assert_eq!(err, "+ a=(1 2)\n");
}

#[test]
fn xtrace_indexed_array_append_forms() {
    let (_, err, _) = run("set -x; a=(1); a+=(2)");
    assert_eq!(err, "+ a=(1)\n+ a+=(2)\n");
    // Append onto an unset variable.
    let (_, err, _) = run("set -x; u+=(z); declare -p u");
    assert_eq!(err, "+ u+=(z)\n+ declare -p u\n");
    // Append onto a scalar: the scalar becomes element 0.
    let (_, err, _) = run("set -x; s=5; s+=(x)");
    assert_eq!(err, "+ s=5\n+ s+=(x)\n");
    // Append onto an empty scalar.
    let (_, err, _) = run("set -x; e=; e+=(x)");
    assert_eq!(err, "+ e=\n+ e+=(x)\n");
}

#[test]
fn xtrace_indexed_array_empty_append() {
    let (_, err, _) = run("set -x; a=(1); a+=()");
    assert_eq!(err, "+ a=(1)\n+ a+=()\n");
}

#[test]
fn xtrace_indexed_array_sparse_append() {
    let (_, err, _) = run("set -x; a=(1); a+=([5]=x)");
    assert_eq!(err, "+ a=(1)\n+ a+=([5]=x)\n");
}

#[test]
fn xtrace_assoc_array_assignment_uses_ellipsis() {
    let (_, err, _) = run("set -x; declare -A m; m=([k]=v)");
    assert_eq!(err, "+ declare -A m\n+ m=(...)\n");
    let (_, err, _) = run("set -x; declare -A m; m+=(x)");
    assert_eq!(err, "+ declare -A m\n+ m+=(...)\n");
}

// ── xtrace of bare assignments ──────────────────────────────────────

#[test]
fn xtrace_bare_assignment_forms() {
    let (_, err, _) = run("set -x; a[0]=x");
    assert_eq!(err, "+ a[0]=x\n");
    let (_, err, _) = run("set -x; a=(1); a[0]+=y");
    assert_eq!(err, "+ a=(1)\n+ a[0]+=y\n");
    let (_, err, _) = run("set -x; s=1; s+=2");
    assert_eq!(err, "+ s=1\n+ s+=2\n");
}

#[test]
fn xtrace_bare_assignment_readonly_error_aborts() {
    let (_, err, code) = run("set -x; readonly r=1; r=2");
    assert_eq!(
        err,
        "+ readonly r=1\n+ r=1\n+ r=2\nrust-bash: line 1: r: readonly variable\n"
    );
    assert_eq!(code, 1);
}

#[test]
fn xtrace_bare_assignment_readonly_error_aborts_posix() {
    let (out, err, code) = run("set -o posix; set -x; readonly r=1; r=2; echo after");
    assert_eq!(
        err,
        "+ readonly r=1\n+ r=1\n+ r=2\nrust-bash: line 1: r: readonly variable\n"
    );
    assert_eq!(out, "");
    assert_eq!(code, 1);
}

#[test]
fn xtrace_bare_assignment_with_redirect() {
    // The bare assignment's (empty) stdout is redirected, truncating /f.
    let (out, err, _) = run("set -x; a=1 > /f; cat /f");
    assert_eq!(out, "");
    assert_eq!(err, "+ a=1\n+ cat /f\n");
}

#[test]
fn xtrace_bare_assignment_exit_code_from_cmdsub() {
    let (out, err, _) = run("set -x; a=$(false); echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(err, "+ false\n+ a=\n+ echo rc=1\n");
}

#[test]
fn xtrace_bare_assignment_cmdsub_stderr() {
    let (out, err, _) = run("set -x; a=$(echo e >&2; echo o); echo $a");
    assert_eq!(out, "o\n");
    assert_eq!(err, "+ echo e\ne\n+ echo o\n+ a=o\n+ echo o\n");
}

// ── Function name validation ────────────────────────────────────────

#[test]
fn function_name_with_expansion_is_rejected() {
    let (_, err, code) = run("$x() { :; }");
    assert_eq!(err, "rust-bash: $x: not a valid function name\n");
    assert_eq!(code, 1);
}
