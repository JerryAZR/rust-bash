//! Coverage tests for src/interpreter/mod.rs (parse-time rewrite helpers,
//! set_variable/set_array_element/set_assoc_element paths), src/interpreter/
//! brace.rs (sequence edge cases), and src/interpreter/analysis.rs (static
//! command collection). All tests drive the public `RustBash` API and pin
//! exact stdout/stderr/exit codes. Suspected divergences from real bash are
//! marked with `DIVERGENCE?` comments.

use rust_bash::{ExecutionLimits, RustBash, RustBashBuilder};

fn shell() -> RustBash {
    RustBashBuilder::new().build().unwrap()
}

/// Run a script in a fresh shell, returning (stdout, stderr, exit_code).
/// A returned `Err` from exec is rendered as exit_code -1 with the error
/// Display string in stderr so error paths can be pinned exactly.
fn run(script: &str) -> (String, String, i32) {
    let mut sh = shell();
    match sh.exec(script) {
        Ok(r) => (r.stdout, r.stderr, r.exit_code),
        Err(e) => (String::new(), format!("ERR:{e}"), -1),
    }
}

// ── Parse retry: assignment-prefixed reserved word ─────────────────

#[test]
fn assignment_prefixed_reserved_word_parses_after_backslash_rewrite() {
    // `A=1 if` fails the first parse (reserved word in command position);
    // the retry escapes it to `\if`, which parses and dispatches as a
    // command named `if`.
    let (out, err, code) = run("A=1 if");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "if: command not found\n", 127)
    );
}

#[test]
fn single_reserved_word_falls_through_all_retries_to_parse_error() {
    // Tokenizes to a single word, so the assignment-prefix rewrite bails
    // out before inspecting tokens.
    let (out, err, code) = run("if");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "parse error: syntax error at end of input\n", 2)
    );
}

#[test]
fn empty_name_assignment_token_is_not_a_simple_assignment_word() {
    // `=x` splits into an empty name at the '=' — rejected by the
    // assignment-word check during the reserved-word retry analysis.
    let (out, err, code) = run("=x | if");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "parse error: syntax error at end of input\n", 2)
    );
}

#[test]
fn digit_leading_assignment_token_is_not_a_simple_assignment_word() {
    // `1A=x` has a name starting with a digit — rejected by the
    // assignment-word check during the reserved-word retry analysis.
    let (out, err, code) = run("1A=x | if");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "parse error: syntax error at end of input\n", 2)
    );
}

// ── Parse retry: assignment-prefixed `[[` ──────────────────────────

#[test]
fn assignment_prefixed_dbracket_is_rewritten_to_quoted_command() {
    // DIVERGENCE? Real bash parses `A=1 [[ x = y ]]` as an extended test
    // with a command-scoped assignment. rust-bash rewrites the brackets to
    // quoted words up front, so it dispatches a command named `[[`.
    let (out, err, code) = run("A=1 [[ x = y ]]");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "[[: command not found\n", 127)
    );
}

#[test]
fn dbracket_retry_after_heredoc_rewrite_discards_first_pass() {
    // The up-front `[[` rewrite is discarded when tokenization fails on the
    // `<<${d}` heredoc delimiter (the heredoc rewrite rewrites from the
    // original input). The parse then fails again and the assignment-prefix
    // retry re-applies the `[[` rewrite — pinned: command `[[` runs.
    let (out, err, code) = run("A=1 [[ x = y ]]\ncat <<${d}\nhi\n${d}");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("hi\n", "[[: command not found\n", 0)
    );
}

#[test]
fn dbracket_retry_success_path_still_fails_on_later_garbage() {
    // The assignment-prefix retry rewrites the `[[` prefix but the trailing
    // `)` keeps the re-parse failing, exercising the retry-failure fallthrough.
    let (out, err, code) = run("A=1 [[ x = y ]])\ncat <<${d}\nhi\n${d}");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "parse error: syntax error at line 1 col 8\n", 2)
    );
}

// ── Parse retry: `[[ -n = ]]` unary-literal-operand rewrite ────────

#[test]
fn extended_test_retry_scans_quoted_escapes_outside_and_inside_tests() {
    // The escaped quotes in `echo "q\"w"` and inside the second `[[ ]]`
    // exercise the quote-skipping logic of the unary-literal-operand
    // rewrite and its extended-test-end scanner.
    let (out, err, code) =
        run("echo \"q\\\"w\"; [[ -n = ]] && echo first; [[ \"x\\\"y\" = \"x\\\"y\" ]] && echo ok");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("q\"w\nfirst\nok\n", "", 0)
    );
}

#[test]
fn extended_test_retry_success_path_still_fails_on_later_garbage() {
    // The `[[ -n = ]]` segment is rewritten but the trailing `)` keeps the
    // re-parse failing, exercising the retry-failure fallthrough.
    let (out, err, code) = run("[[ -n = ]]; )");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "parse error: syntax error at line 1 col 11\n", 2)
    );
}

#[test]
fn unterminated_extended_test_is_a_parse_error() {
    // No closing `]]` — the extended-test-end scanner comes up empty.
    let (out, err, code) = run("[[ -n =");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "parse error: syntax error at end of input\n", 2)
    );
}

// ── Legacy ksh `${ ...; }` rewrite ──────────────────────────────────

#[test]
fn legacy_ksh_empty_pipe_body_is_left_unrewritten() {
    // `${|}` has an empty body after the `|`, so the legacy-ksh rewrite
    // declines; expansion later rejects it as a bad substitution.
    let (out, err, code) = run("echo ${|}");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "rust-bash: ${|}: bad substitution\n", 1)
    );
}

#[test]
fn legacy_ksh_empty_command_body_is_left_unrewritten() {
    // `${ ;}` trims to an empty command body, so the legacy-ksh rewrite
    // declines.
    // DIVERGENCE? Real bash reports ``${ ;}`: bad substitution'' here;
    // rust-bash leaves the token unrewritten and prints it literally with
    // the inner whitespace collapsed by word splitting.
    let (out, err, code) = run("echo ${ ;}");
    assert_eq!((out.as_str(), err.as_str(), code), ("${;}\n", "", 0));
}

#[test]
fn legacy_ksh_case_with_parenthesized_patterns_needs_no_paren_insert() {
    // The legacy-ksh normalizer inserts `(` before unparenthesized case
    // patterns; a body that is already parenthesized skips the insertion.
    // DIVERGENCE? Real bash rejects `${ ...; }` as a bad substitution; the
    // ksh-style rewrite is an intentional rust-bash feature.
    let (out, err, code) = run("x=a; echo ${ case $x in (a) REPLY=hit;; esac; }");
    assert_eq!((out.as_str(), err.as_str(), code), ("\n", "", 0));
}

#[test]
fn legacy_ksh_case_without_in_keyword_falls_through_normalizer() {
    // A `case` body with no ` in ` gives the normalizer nothing to rewrite;
    // the result parses (`$(case esac)`) but the command substitution's
    // inner re-parse fails at expansion time.
    let (out, err, code) = run("echo ${ case esac; }");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "ERR:parse error: syntax error at end of input", -1)
    );
}

// ── Heredoc expansion-like delimiter rewrite ────────────────────────

#[test]
fn heredoc_delimiter_rewrite_handles_dash_spaces_and_plain_delimiters() {
    // One script containing `<<${a}` (forces the rewrite pass), `<<-${b}`
    // (tab-stripping dash form), `<<  ${c}` (whitespace before delimiter),
    // and a plain `<<-EOF` (left untouched by the rewrite).
    let script = "cat <<${a}\none\n${a}\ncat <<-${b}\ntwo\n${b}\ncat <<  ${c}\nthree\n${c}\ncat <<-EOF\nfour\nEOF";
    let (out, err, code) = run(script);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("one\ntwo\nthree\nfour\n", "", 0)
    );
}

// ── set_variable: limits, nameref subscripts, SECONDS ───────────────

#[test]
fn append_assignment_enforces_max_string_length_in_set_variable() {
    // The RHS pieces are each under the limit, but the appended result
    // overflows it inside set_variable.
    let limits = ExecutionLimits {
        max_string_length: 4,
        ..Default::default()
    };
    let mut sh = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap();
    let err = sh.exec("x=aaa; x+=bb").unwrap_err();
    assert_eq!(
        err.to_string(),
        "limit exceeded: max_string_length (5) exceeded limit (4)"
    );
}

#[test]
fn nameref_to_readonly_array_element_assignment_fails() {
    let (out, err, code) = run("arr=(1 2); readonly arr; declare -n ref=arr[0]; ref=x");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "rust-bash: line 1: arr: readonly variable\n", 1)
    );
}

#[test]
fn nameref_to_negative_array_index_resolves_from_max_key() {
    let (out, err, code) = run("arr=(a b c); declare -n ref=arr[-1]; ref=LAST; echo ${arr[2]}");
    assert_eq!((out.as_str(), err.as_str(), code), ("LAST\n", "", 0));
}

#[test]
fn nameref_to_out_of_range_negative_array_index_clamps_to_zero() {
    // DIVERGENCE? Real bash reports `arr[-5]: bad array subscript` when the
    // resolved index underflows; rust-bash clamps the resolved index to 0.
    let (out, err, code) = run("arr=(a); declare -n ref=arr[-5]; ref=X; echo ${arr[0]}");
    assert_eq!((out.as_str(), err.as_str(), code), ("X\n", "", 0));
}

#[test]
fn nameref_to_scalar_subscript_zero_assigns_the_scalar() {
    // Subscript 0 targets the scalar itself; subscript 3 is a silent no-op
    // on a scalar (bash converts to an indexed array but leaves $s, i.e.
    // element 0, untouched — same observable result).
    let (out, err, code) =
        run("s=orig; declare -n ref=s[0]; ref=new; echo $s; declare -n r2=s[3]; r2=zzz; echo $s");
    assert_eq!((out.as_str(), err.as_str(), code), ("new\nnew\n", "", 0));
}

#[test]
fn non_numeric_seconds_assignment_resets_the_timer() {
    let (out, err, code) = run("SECONDS=abc; if (( SECONDS < 5 )); then echo small; fi");
    assert_eq!((out.as_str(), err.as_str(), code), ("small\n", "", 0));
}

// ── set_assoc_element via nameref/assoc combination ────────────────

#[test]
fn assoc_compound_assignment_on_nameref_errors_with_empty_target() {
    // `declare -A x=([k]=v)` converts the nameref `x` to an (empty)
    // associative value while keeping the NAMEREF attribute; resolving the
    // nameref then yields an empty target name and the element insert fails.
    // DIVERGENCE? Real bash rejects `declare -A` on a nameref variable
    // outright; rust-bash attempts the assignment and reports the failure
    // with an empty variable name in the message.
    let (out, err, code) = run("declare -n x=t; declare -A x=([k]=v); echo after");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("after\n", "rust-bash: : not an associative array\n", 0)
    );
}

#[test]
fn assoc_element_assignment_through_nameref_to_assoc_valued_nameref_fails() {
    // `declare -A x` (no value) on a nameref likewise leaves `x`
    // NAMEREF + empty-assoc-valued, so `x[k]=v` resolves to an empty target.
    // DIVERGENCE? As above: real bash rejects the nameref/array combination
    // instead of emitting an empty-named error.
    let (out, err, code) = run("declare -n x=t; t=s; declare -A x; x[k]=v");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "rust-bash: line 1: : not an associative array\n", 1)
    );
}

// ── Attribute transforms on array element assignment ────────────────

#[test]
fn indexed_array_element_assignment_applies_case_transforms() {
    let (out, err, code) = run(
        "declare -la arr; arr[0]=ABC; echo ${arr[0]}; declare -ua arr2; arr2[0]=abc; echo ${arr2[0]}",
    );
    assert_eq!((out.as_str(), err.as_str(), code), ("abc\nABC\n", "", 0));
}

#[test]
fn assoc_array_element_assignment_applies_integer_and_case_transforms() {
    let (out, err, code) = run(
        "declare -Ai m; m[k]=1+2; echo ${m[k]}; declare -Al m2; m2[k]=ABC; echo ${m2[k]}; declare -Au m3; m3[k]=abc; echo ${m3[k]}",
    );
    assert_eq!((out.as_str(), err.as_str(), code), ("3\nabc\nABC\n", "", 0));
}

// ── analyze_commands (analysis.rs) ──────────────────────────────────

#[test]
fn analyze_commands_collects_or_list_rhs() {
    let sh = shell();
    let a = sh.analyze_commands("alpha || beta").unwrap();
    assert_eq!(a.commands, vec!["alpha", "beta"]);
}

#[test]
fn analyze_commands_collects_arithmetic_for_body() {
    let sh = shell();
    let a = sh
        .analyze_commands("for ((i=0;i<3;i++)); do gamma; done")
        .unwrap();
    assert_eq!(a.commands, vec!["gamma"]);
}

// ── Brace expansion (brace.rs) ──────────────────────────────────────

#[test]
fn brace_body_may_contain_backtick_substitutions() {
    let (out, err, code) = run("echo {`echo a`,b}");
    assert_eq!((out.as_str(), err.as_str(), code), ("a b\n", "", 0));
}

#[test]
fn numeric_sequence_with_non_numeric_step_is_left_literal() {
    let (out, err, code) = run("echo {1..10..x}");
    assert_eq!((out.as_str(), err.as_str(), code), ("{1..10..x}\n", "", 0));
}

#[test]
fn numeric_sequence_stops_at_i64_max_without_overflow() {
    let (out, err, code) = run("echo {9223372036854775806..9223372036854775807}");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("9223372036854775806 9223372036854775807\n", "", 0)
    );
}

#[test]
fn descending_numeric_sequence_stops_at_i64_min_without_overflow() {
    let (out, err, code) = run("echo {-9223372036854775807..-9223372036854775808}");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("-9223372036854775807 -9223372036854775808\n", "", 0)
    );
}

#[test]
fn char_sequence_with_non_numeric_step_is_left_literal() {
    let (out, err, code) = run("echo {a..z..x}");
    assert_eq!((out.as_str(), err.as_str(), code), ("{a..z..x}\n", "", 0));
}

#[test]
fn char_sequence_with_u32_overflowing_step_yields_first_char() {
    // 97 + 4294967295 overflows u32, so the sequence stops after 'a'.
    let (out, err, code) = run("echo {a..z..4294967295}");
    assert_eq!((out.as_str(), err.as_str(), code), ("a\n", "", 0));
}

#[test]
fn descending_char_sequence_with_i64_min_step_stops_immediately() {
    // step parses as i64::MIN; the descending checked subtraction overflows
    // and the sequence stops after 'z'.
    let (out, err, code) = run("echo {z..a..-9223372036854775808}");
    assert_eq!((out.as_str(), err.as_str(), code), ("z\n", "", 0));
}

#[test]
fn negative_values_are_zero_padded_in_sequences() {
    let (out, err, code) = run("echo {-05..-03}");
    assert_eq!((out.as_str(), err.as_str(), code), ("-05 -04 -03\n", "", 0));
}
