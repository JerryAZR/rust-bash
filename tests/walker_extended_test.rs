//! Behavioral tests for `[[ ]]` extended test evaluation and its xtrace
//! rendering in the walker, driven through the public `RustBash` API.

use rust_bash::RustBashBuilder;

fn shell() -> rust_bash::RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> (String, String, i32) {
    let mut sh = shell();
    let r = sh.exec(script).unwrap();
    (r.stdout, r.stderr, r.exit_code)
}

// ── xtrace rendering of compound expressions ────────────────────────

#[test]
fn xtrace_and_or_not_rendering() {
    let (out, err, _) = run("set -x; [[ -n abc && -z '' || ! -d /x ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
    assert_eq!(err, "+ [[ -n abc && -z  || ! -d /x ]]\n+ echo rc=0\n");
}

#[test]
fn xtrace_parenthesized_rendering() {
    let (_, err, _) = run("set -x; [[ ( -n a ) && 1 -eq 1 ]]; echo rc=$?");
    assert_eq!(err, "+ [[ -n a && 1 -eq 1 ]]\n+ echo rc=0\n");
}

#[test]
fn xtrace_binary_predicates_rendering() {
    let cases: &[(&str, &str)] = &[
        ("[[ abc == a* ]]", "+ [[ abc == a* ]]\n+ echo rc=0\n"),
        ("[[ abc != z* ]]", "+ [[ abc != z* ]]\n+ echo rc=0\n"),
        ("[[ abc =~ ^a ]]", "+ [[ abc =~ ^a ]]\n+ echo rc=0\n"),
        ("[[ 1 -ne 2 ]]", "+ [[ 1 -ne 2 ]]\n+ echo rc=0\n"),
        ("[[ 1 -lt 2 ]]", "+ [[ 1 -lt 2 ]]\n+ echo rc=0\n"),
        ("[[ 2 -gt 1 ]]", "+ [[ 2 -gt 1 ]]\n+ echo rc=0\n"),
        ("[[ 1 -le 1 ]]", "+ [[ 1 -le 1 ]]\n+ echo rc=0\n"),
        ("[[ 1 -ge 1 ]]", "+ [[ 1 -ge 1 ]]\n+ echo rc=0\n"),
        // `<` / `>` have no dedicated trace spelling and render as `?`.
        ("[[ a < b ]]", "+ [[ a ? b ]]\n+ echo rc=0\n"),
        ("[[ b > a ]]", "+ [[ b ? a ]]\n+ echo rc=0\n"),
    ];
    for (test_expr, expected_err) in cases {
        let (out, err, _) = run(&format!("set -x; {test_expr}; echo rc=$?"));
        assert_eq!(out, "rc=0\n", "expr: {test_expr}");
        assert_eq!(err, *expected_err, "expr: {test_expr}");
    }
}

#[test]
fn xtrace_file_binary_predicates_rendering() {
    let (out, err, _) = run(
        "set -x; touch /a /b 2>/dev/null; [[ /a -ef /b ]]; echo rc=$?; \
         [[ /a -nt /b ]]; echo rc=$?; [[ /a -ot /b ]]; echo rc=$?",
    );
    assert_eq!(out, "rc=1\nrc=1\nrc=0\n");
    assert_eq!(
        err,
        "+ [[ /a -ef /b ]]\n+ echo rc=1\n+ [[ /a -nt /b ]]\n+ echo rc=1\n\
         + [[ /a -ot /b ]]\n+ echo rc=0\n"
    );
}

#[test]
fn xtrace_unary_predicates_rendering() {
    // Each predicate arm of format_unary_pred is exercised; all of these
    // evaluate to false on the VFS (exit 1).
    let preds = [
        "-a", "-b", "-c", "-f", "-g", "-h", "-k", "-p", "-r", "-s", "-t", "-u", "-w", "-x", "-G",
        "-N", "-O", "-S",
    ];
    for pred in preds {
        let operand = if pred == "-t" { "0" } else { "/nope" };
        let (out, err, _) = run(&format!("set -x; [[ {pred} {operand} ]]; echo rc=$?"));
        assert_eq!(out, "rc=1\n", "pred: {pred}");
        assert_eq!(
            err,
            format!("+ [[ {pred} {operand} ]]\n+ echo rc=1\n"),
            "pred: {pred}"
        );
    }
}

#[test]
fn xtrace_unary_option_and_variable_predicates() {
    let (out, err, _) = run("set -x; [[ -o errexit ]]; echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(err, "+ [[ -o errexit ]]\n+ echo rc=1\n");
    let (out, err, _) = run("set -x; v=1; [[ -v v ]]; echo rc=$?; [[ -R v ]]; echo rc=$?");
    assert_eq!(out, "rc=0\nrc=1\n");
    assert_eq!(
        err,
        "+ v=1\n+ [[ -v v ]]\n+ echo rc=0\n+ [[ -R v ]]\n+ echo rc=1\n"
    );
}

// ── Arithmetic predicate -ne ────────────────────────────────────────

#[test]
fn arithmetic_not_equal() {
    let (out, _, _) = run("[[ 1 -ne 2 ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
    let (out, _, _) = run("[[ 1 -ne 1 ]]; echo rc=$?");
    assert_eq!(out, "rc=1\n");
}

// ── nocasematch / extglob pattern matching ──────────────────────────

#[test]
fn nocasematch_glob_and_literal() {
    let (out, _, _) = run("shopt -s nocasematch; [[ ABC == a* ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
    let (out, _, _) = run("shopt -s nocasematch; [[ ABC == 'abc' ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
    let (out, _, _) = run("shopt -s nocasematch; [[ ABC != z* ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
    let (out, _, _) = run("shopt -s nocasematch; [[ ABC != 'zzz' ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
}

#[test]
fn nocasematch_extglob_nonmatch_matches_bash() {
    // Verified against real bash 5.2: `@(a|b)c` expands to "ac" or "bc",
    // so neither abc nor ABC matches (rc=1) — rust-bash is correct here.
    let (out, _, _) = run("shopt -s nocasematch extglob; [[ ABC == @(a|b)c ]]; echo rc=$?");
    assert_eq!(out, "rc=1\n");
    // The negated form correspondingly succeeds.
    let (out, _, _) = run("shopt -s nocasematch extglob; [[ ABC != @(z) ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
}

#[test]
fn extglob_exactly_one_semantics_match_bash() {
    // Verified against real bash 5.2: `@` is "exactly one of", so
    // `@(a|b)c` does not match abc (rc=1) — rust-bash is correct here.
    let (out, _, _) = run("shopt -s extglob; [[ abc == @(a|b)c ]]; echo rc=$?");
    assert_eq!(out, "rc=1\n");
    let (out, _, _) = run("shopt -s extglob; [[ abc != @(z) ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
}

#[test]
fn nocasematch_other_predicates_delegate() {
    let (out, _, _) = run("shopt -s nocasematch; [[ /a -nt /b ]]; echo rc=$?");
    assert_eq!(out, "rc=1\n");
}

// ── [[ -v ]] variable-set checks ────────────────────────────────────

#[test]
fn v_flag_on_array_at() {
    let (out, _, _) = run("declare -a a=(1); [[ -v 'a[@]' ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
    let (out, _, _) = run("declare -A m=([k]=v); [[ -v 'm[@]' ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
    let (out, _, _) = run("declare -a a; [[ -v 'a[@]' ]]; echo rc=$?");
    assert_eq!(out, "rc=1\n");
}

#[test]
fn v_flag_on_scalar_subscript() {
    let (out, _, _) = run("s=1; [[ -v 's[0]' ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
    let (out, _, _) = run("s=1; [[ -v 's[5]' ]]; echo rc=$?");
    assert_eq!(out, "rc=1\n");
}

#[test]
fn v_flag_subscript_with_side_effect_assignment() {
    // The subscript is evaluated as arithmetic; `a=2` writes element 0 of
    // the indexed array and the check then tests index 2. Pinned actual
    // behavior.
    let (out, _, _) = run("declare -a a=(1); [[ -v 'a[a=2]' ]]; echo rc=$?; declare -p a");
    assert_eq!(out, "rc=1\ndeclare -a a=([0]=\"2\")\n");
}

#[test]
fn v_flag_on_missing_variable_subscript() {
    let (out, _, _) = run("[[ -v 'missing[0]' ]]; echo rc=$?");
    assert_eq!(out, "rc=1\n");
}

#[test]
fn v_flag_negative_subscript() {
    let (out, _, _) = run("declare -a a=(1); [[ -v 'a[-1]' ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
}

#[test]
fn v_flag_out_of_range_negative_subscript_warns() {
    let (out, err, _) = run("declare -a a=(1); [[ -v 'a[-9]' ]]; echo rc=$?");
    assert_eq!(out, "rc=1\n");
    assert_eq!(err, "rust-bash: line 1: a: bad array subscript\n");
}

#[test]
fn v_flag_assoc_key() {
    let (out, _, _) = run("declare -A m=([k]=v); [[ -v 'm[k]' ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
}

#[test]
fn regex_quoted_pattern_trace_and_substring_predicate() {
    // A quoted =~ right side uses the substring predicate, traced as =~.
    let (out, err, _) = run("set -x; [[ abc =~ 'b' ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
    assert_eq!(err, "+ [[ abc =~ b ]]\n+ echo rc=0\n");
}

#[test]
fn v_flag_scalar_with_at_subscript_is_false() {
    let (out, _, _) = run("s=1; [[ -v 's[@]' ]]; echo rc=$?");
    assert_eq!(out, "rc=1\n");
}

#[test]
fn regex_partial_double_quoting_escapes_literals() {
    // Partially quoted pattern: the quoted `\c` collapses to a literal c.
    let (out, _, _) = run("[[ aqc =~ a\"q\\c\" ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
}

#[test]
fn regex_nested_brace_variable_expansion() {
    // ${v:-x{y}} contains a nested brace; the expanded `{` reaches the
    // regex engine unescaped → invalid regex.
    let (out, err, code) = run("unset v; [[ 'ax{yc' =~ a${v:-x{y}c ]]; echo rc=$?");
    assert_eq!(out, "rc=2\n");
    assert_eq!(code, 0);
    assert!(
        err.starts_with("rust-bash: invalid regex 'ax{yc'"),
        "err: {err}"
    );
}

// ── Regex matching ──────────────────────────────────────────────────

#[test]
fn regex_escaped_char_in_double_quoted_portion() {
    // `\q` inside the double-quoted portion collapses to a literal q.
    let (out, _, _) = run(r#"[[ aqc =~ "a\qc" ]]; echo rc=$?"#);
    assert_eq!(out, "rc=1\n");
    let (out, _, _) = run(r#"[[ aqc =~ "a\qc" ]] || [[ aqc =~ aqc ]]; echo rc=$?"#);
    assert_eq!(out, "rc=0\n");
}

#[test]
fn regex_with_braced_variable_expansion() {
    let (out, _, _) = run("v=x; [[ axc =~ a${v}c ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
}

#[test]
fn regex_with_nested_brace_variable_expansion() {
    let (out, err, code) = run("v='{'; [[ 'a{c' =~ a${v}c ]]; echo rc=$?");
    // The expanded `{` reaches the regex engine unescaped → invalid regex.
    assert_eq!(out, "rc=2\n");
    assert_eq!(code, 0);
    assert!(
        err.starts_with("rust-bash: invalid regex 'a{c'"),
        "err: {err}"
    );
}

#[test]
fn regex_fully_quoted_pattern_is_literal() {
    let (out, _, _) = run("[[ 'a.c' =~ 'a.c' ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
}

#[test]
fn regex_tilde_pattern_is_escaped_literal() {
    let (out, _, _) = run("[[ abc =~ ~ ]]; echo rc=$?");
    assert_eq!(out, "rc=1\n");
}

#[test]
fn regex_nocasematch_makes_case_insensitive() {
    let (out, _, _) = run("shopt -s nocasematch; [[ ABC =~ ^a ]]; echo rc=$?");
    assert_eq!(out, "rc=0\n");
}

#[test]
fn regex_capture_groups_populate_bash_rematch() {
    let (out, _, _) = run("[[ abc =~ (b) ]]; echo ${BASH_REMATCH[0]} ${BASH_REMATCH[1]}");
    assert_eq!(out, "b b\n");
}

#[test]
fn extended_test_ampersand_redirect_pre_truncates() {
    // `&>` on a compound command pre-truncates before the body runs (bash
    // redirect ordering); `&>>` appends instead.
    let (out, _, code) = run("echo old > /f; [[ a == a ]] &> /f; wc -c < /f");
    assert_eq!((out.as_str(), code), ("0\n", 0));
    let (out, _, _) = run("echo old > /g; [[ a == a ]] &>> /g; wc -c < /g");
    assert_eq!(out, "4\n");
}
