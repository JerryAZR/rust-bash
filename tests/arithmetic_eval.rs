//! Behavioral tests for the arithmetic evaluator (`$(( ))`, `(( ))`, `let`)
//! driven through the public `RustBash` API.

use rust_bash::RustBashBuilder;

fn shell() -> rust_bash::RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> (String, String, i32) {
    let mut sh = shell();
    let r = sh.exec(script).unwrap();
    (r.stdout, r.stderr, r.exit_code)
}

// ── Tokenizer: $-prefixed idents ────────────────────────────────────

#[test]
fn positional_and_special_dollar_idents() {
    let (out, err, code) = run("set -- 10 20; echo $(( $1 + $2 )) $(( $# ))");
    assert_eq!(out, "30 2\n");
    assert_eq!(err, "");
    assert_eq!(code, 0);
}

#[test]
fn dollar_zero_reads_shell_name_as_zero() {
    // $0 is "rust-bash", which does not parse as a number → 0.
    let (out, _, _) = run("echo $(( $0 + 7 ))");
    assert_eq!(out, "7\n");
}

#[test]
fn dollar_question_reads_last_exit_code() {
    let (out, _, _) = run("false; echo $(( $? + 1 ))");
    assert_eq!(out, "2\n");
}

#[test]
fn escaped_dollar_idents_reach_the_tokenizer() {
    // A backslash-escaped `$` survives expansion and is handled by the
    // arithmetic tokenizer itself (positionals, $#, $?, $0).
    let (out, _, _) = run("set -- 10 20; echo $(( \\$1 + \\$2 ))");
    assert_eq!(out, "30\n");
    let (out, _, _) = run("set -- 10 20; echo $(( \\$# )) $(( \\$0 + 7 ))");
    assert_eq!(out, "2 7\n");
    let (out, _, _) = run("false; echo $(( \\$? + 1 ))");
    assert_eq!(out, "2\n");
}

#[test]
fn lone_dollar_is_skipped_by_tokenizer() {
    // `$` followed by a non-ident character produces no token.
    let (out, _, code) = run("echo $(( $ + 1 ))");
    assert_eq!(out, "1\n");
    assert_eq!(code, 0);
}

// ── Compound assignment operators ───────────────────────────────────

#[test]
fn compound_assignments() {
    let cases = [
        ("x=8; echo $(( x -= 3 )) $x", "5 5\n"),
        ("x=8; echo $(( x *= 3 )) $x", "24 24\n"),
        ("x=8; echo $(( x <<= 2 )) $x", "32 32\n"),
        ("x=8; echo $(( x >>= 1 )) $x", "4 4\n"),
        ("x=8; echo $(( x %= 3 )) $x", "2 2\n"),
        ("x=8; echo $(( x &= 6 )) $x", "0 0\n"),
        ("x=8; echo $(( x |= 3 )) $x", "11 11\n"),
        ("x=8; echo $(( x ^= 5 )) $x", "13 13\n"),
    ];
    for (script, expected) in cases {
        let (out, err, code) = run(script);
        assert_eq!(out, expected, "script: {script}");
        assert_eq!(err, "", "script: {script}");
        assert_eq!(code, 0, "script: {script}");
    }
}

#[test]
fn compound_divide_by_zero_is_nonfatal_error() {
    let (out, err, code) = run("x=8; echo $(( x /= 0 )); echo after");
    assert_eq!(out, "after\n");
    assert_eq!(err, "rust-bash: line 1: arithmetic: division by zero\n");
    assert_eq!(code, 0);
}

#[test]
fn compound_modulo_by_zero_is_nonfatal_error() {
    let (out, err, _) = run("x=8; echo $(( x %= 0 )); echo after");
    assert_eq!(out, "after\n");
    assert_eq!(err, "rust-bash: line 1: arithmetic: division by zero\n");
}

// ── Number literals ─────────────────────────────────────────────────

#[test]
fn invalid_hex_number_empty_digits() {
    let (_, err, code) = run("echo $(( 0x ))");
    assert_eq!(err, "rust-bash: line 1: arithmetic: invalid hex number\n");
    assert_eq!(code, 1);
}

#[test]
fn invalid_hex_number_overflow() {
    let (_, err, _) = run("echo $(( 0xFFFFFFFFFFFFFFFFFFFF ))");
    assert_eq!(
        err,
        "rust-bash: line 1: arithmetic: invalid hex number `0xFFFFFFFFFFFFFFFFFFFF`\n"
    );
}

#[test]
fn invalid_octal_number() {
    let (_, err, _) = run("echo $(( 09 ))");
    assert_eq!(
        err,
        "rust-bash: line 1: arithmetic: invalid octal number `09`\n"
    );
}

#[test]
fn invalid_arithmetic_base() {
    let (_, err, _) = run("echo $(( 1#10 ))");
    assert_eq!(
        err,
        "rust-bash: line 1: arithmetic: invalid arithmetic base: 1\n"
    );
    let (_, err, _) = run("echo $(( 65#10 ))");
    assert_eq!(
        err,
        "rust-bash: line 1: arithmetic: invalid arithmetic base: 65\n"
    );
}

#[test]
fn digit_too_great_for_base() {
    let (_, err, _) = run("echo $(( 2#102 ))");
    assert_eq!(
        err,
        "rust-bash: line 1: arithmetic: value too great for base: 102 (base 2)\n"
    );
}

#[test]
fn base_64_digits() {
    let (out, _, _) = run("echo $(( 64#_ )) $(( 64#@ )) $(( 16#ff ))");
    assert_eq!(out, "63 62 255\n");
}

#[test]
fn strict_arith_rejects_zero_padded_base_constant() {
    let (_, err, _) = run("shopt -s strict_arith; echo $(( 07#9 ))");
    assert_eq!(
        err,
        "rust-bash: line 1: arithmetic: invalid base constant `07#`\n"
    );
}

// ── Quoted strings inside arithmetic ────────────────────────────────

#[test]
fn double_quoted_number_evaluates() {
    let (out, _, code) = run(r#"echo $(( "3" + 1 ))"#);
    assert_eq!(out, "4\n");
    assert_eq!(code, 0);
}

#[test]
fn double_quoted_assoc_key() {
    let (out, _, _) = run(r#"declare -A m=([k]=3); echo $(( m["k"] + 1 ))"#);
    assert_eq!(out, "4\n");
}

#[test]
fn escape_inside_double_quoted_string() {
    // The tokenizer skips `\x` escapes inside "..."; the inner re-tokenize
    // then fails on the backslash. Pinned actual behavior.
    let (_, err, code) = run(r#"echo $(( "a\"b" + 0 ))"#);
    assert_eq!(
        err,
        "rust-bash: line 1: arithmetic: unexpected character `\\`\n"
    );
    assert_eq!(code, 1);
}

#[test]
fn single_quote_inside_double_quotes_errors() {
    let (_, err, _) = run(r#"echo $(( "'" ))"#);
    assert_eq!(
        err,
        "rust-bash: line 1: arithmetic: syntax error: operand expected\n"
    );
}

// ── Ternary short-circuit skipping ──────────────────────────────────

#[test]
fn nested_ternary_in_skipped_false_branch() {
    let (out, _, _) = run("echo $(( 1 ? 2 : 0 ? 3 : 4 ))");
    assert_eq!(out, "2\n");
}

#[test]
fn nested_ternary_in_skipped_true_branch() {
    let (out, _, _) = run("echo $(( 0 ? 0 ? 1 : 2 : 3 ))");
    assert_eq!(out, "3\n");
}

#[test]
fn nested_ternary_inside_parens_divergence() {
    // DIVERGENCE (suspected): real bash prints 2 here. rust-bash's
    // skip_ternary_branch consumes the closing `)` while skipping the
    // nested false branch, so the parenthesized parse fails.
    let (_, err, code) = run("echo $(( (1 ? 2 : 0 ? 3 : 4) + 0 ))");
    assert_eq!(err, "rust-bash: line 1: arithmetic: expected RParen\n");
    assert_eq!(code, 1);
}

// ── Logical operator short-circuit skipping ─────────────────────────

#[test]
fn logical_or_skips_array_subscript_rhs() {
    // RHS is skipped without evaluation; a[0] keeps its value.
    let (out, _, _) = run("declare -a a=(7); echo $(( 1 || a[0] )) ${a[0]}");
    assert_eq!(out, "1 7\n");
}

#[test]
fn logical_or_skip_stops_at_nested_and() {
    let (out, _, _) = run("y=3; z=4; echo $(( 1 || y && z ))");
    assert_eq!(out, "1\n");
}

#[test]
fn logical_or_skip_stops_at_ternary_comma_and_rparen() {
    let (out, _, _) = run("echo $(( 1 || x ? 2 : 3 ))");
    assert_eq!(out, "2\n");
    let (out, _, _) = run("echo $(( 1 || x, 5 ))");
    assert_eq!(out, "5\n");
    let (out, _, _) = run("echo $(( (1 || x) + 1 ))");
    assert_eq!(out, "2\n");
}

#[test]
fn logical_and_skips_rhs_and_stops_at_operators() {
    let (out, _, _) = run("echo $(( 0 && x && y ))");
    assert_eq!(out, "0\n");
    let (out, _, _) = run("echo $(( 0 && x || 1 ))");
    assert_eq!(out, "1\n");
}

// ── Increment/decrement on array elements ───────────────────────────

#[test]
fn postfix_increment_with_nested_subscript() {
    let (out, _, _) = run("declare -a a=(1 2 3); declare -a b=(1); echo $(( a[b[0]]++ )) ${a[1]}");
    assert_eq!(out, "2 3\n");
}

#[test]
fn postfix_fallthrough_when_not_inc_dec() {
    let (out, _, _) = run("declare -a a=(5); echo $(( a[0] + 1 ))");
    assert_eq!(out, "6\n");
}

#[test]
fn postfix_fallthrough_without_inc_dec() {
    // `a[0] + 1` parses the subscript but finds no postfix ++/--.
    let (out, _, _) = run("declare -a a=(5); let 'x = a[0] + 1'; echo $x");
    assert_eq!(out, "6\n");
    let (out, _, _) = run("declare -a a=(5); (( x = a[0] + 1 )); echo $x");
    assert_eq!(out, "6\n");
}

#[test]
fn pre_inc_dec_on_array_elements() {
    let (out, _, _) = run("declare -a a=(5); echo $(( ++a[0] )) ${a[0]}");
    assert_eq!(out, "6 6\n");
    let (out, _, _) = run("declare -a a=(5); echo $(( --a[0] )) ${a[0]}");
    assert_eq!(out, "4 4\n");
}

#[test]
fn postfix_increment_on_assoc_element() {
    let (out, _, _) = run("declare -A m=([k]=3); echo $(( m[k]++ )) ${m[k]}");
    assert_eq!(out, "3 4\n");
}

#[test]
fn pre_increment_requires_variable_name() {
    let (_, err, _) = run("echo $(( ++5 ))");
    assert_eq!(
        err,
        "rust-bash: line 1: arithmetic: expected variable name\n"
    );
}

// ── Array subscript edge cases ──────────────────────────────────────

#[test]
fn empty_subscript_is_bad_array_subscript() {
    let (_, err, _) = run("echo $(( a[] ))");
    assert_eq!(err, "rust-bash: line 1: a: bad array subscript\n");
}

#[test]
fn empty_subscript_write_is_bad_array_subscript() {
    let (_, err, _) = run("echo $(( a[] = 3 )); echo after");
    assert_eq!(err, "rust-bash: line 1: a: bad array subscript\n");
}

#[test]
fn empty_subscript_postfix_is_bad_array_subscript() {
    let (_, err, _) = run("declare -a a=(5); echo $(( a[]++ )); echo after");
    assert_eq!(err, "rust-bash: line 1: a: bad array subscript\n");
}

#[test]
fn nounset_unbound_array_element() {
    let (_, err, _) = run("set -u; echo $(( a[0] ))");
    assert_eq!(err, "rust-bash: line 1: a[0]: unbound variable\n");
}

#[test]
fn write_with_negative_index_on_scalar() {
    let (out, _, _) = run("s=5; echo $(( s[-1]=9 )) $s");
    assert_eq!(out, "9 9\n");
}

#[test]
fn write_with_negative_index_unset_var_errors() {
    let (_, err, _) = run("echo $(( u[-1]=3 )); echo after");
    assert_eq!(err, "rust-bash: line 1: u: bad array subscript\n");
}

#[test]
fn write_with_out_of_range_negative_index_errors() {
    let (_, err, _) = run("declare -a a=(1); echo $(( a[-5]=3 )); echo after");
    assert_eq!(err, "rust-bash: line 1: a: bad array subscript\n");
}

#[test]
fn recursive_array_element_evaluation_errors() {
    let mut sh = shell();
    let r = sh.exec("declare -a a; a[0]='a[0]'; echo $(( a[0] ))");
    assert!(matches!(
        r,
        Err(rust_bash::RustBashError::Execution(ref msg))
            if msg == "a[0]: recursive evaluation depth exceeded"
    ));
}

#[test]
fn deep_variable_indirection_chain_bottoms_out() {
    // 11 hops of name indirection exceeds the recursion guard for
    // expression-valued variables, so the innermost `5+5` is not evaluated
    // and the chain bottoms out at 0.
    let (out, _, _) = run(
        "x1=x2; x2=x3; x3=x4; x4=x5; x5=x6; x6=x7; x7=x8; x8=x9; x9=x10; \
         x10=x11; x11='5+5'; echo $(( x1 ))",
    );
    assert_eq!(out, "0\n");
}

#[test]
fn subscript_assignment_writes_element_zero_of_array() {
    // `a[a=2]` evaluates the subscript `a=2`, which writes element 0 of the
    // indexed array (set_variable on an array name writes index 0), then
    // reads index 2. Pinned actual behavior.
    let (out, _, _) = run("declare -a a=(1 2); echo $(( a[a=2] )); declare -p a");
    assert_eq!(out, "0\ndeclare -a a=([0]=\"2\" [1]=\"2\")\n");
}

#[test]
fn scalar_subscript_assignment_reads_updated_scalar() {
    // `s[s=5]` assigns s=5 in the subscript, then reads s[5] of a scalar,
    // which is empty → 0. Pinned actual behavior.
    let (out, _, _) = run("s=3; echo $(( s[s=5] )) $s");
    assert_eq!(out, "0 5\n");
}

// ── Special variables in arithmetic ─────────────────────────────────

#[test]
fn special_variables() {
    let (out, _, _) = run("echo $(( LINENO + 0 ))");
    assert_eq!(out, "1\n");
    let (out, _, _) = run("echo $(( BASH_LINENO + 0 ))");
    assert_eq!(out, "0\n");
    let (out, _, _) = run("echo $(( SECONDS + 0 ))");
    assert_eq!(out, "0\n");
}
