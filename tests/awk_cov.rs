//! Coverage-triage tests for the awk subsystem (`src/commands/awk/`).
//!
//! Every test here exists to cover a previously-uncovered region of the awk
//! lexer, parser, runtime, or command-line handling: malformed programs
//! (lexer/parser error paths), rarely-used language features, and edge-case
//! built-in behavior. Where behavior diverges from real awk (gawk/mawk), the
//! actual behavior is pinned with a comment (runtime behavior is intentionally
//! not changed).

use rust_bash::{ExecResult, ExecutionLimits, RustBash, RustBashBuilder};

fn shell() -> RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> ExecResult {
    shell().exec(script).unwrap()
}

fn run_with_limits(script: &str, limits: ExecutionLimits) -> ExecResult {
    RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap()
        .exec(script)
        .unwrap()
}

// ── mod.rs: command-line handling ────────────────────────────────────

#[test]
fn option_f_missing_argument() {
    let r = run("awk -F");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "awk: option -F requires an argument\n");
}

#[test]
fn option_v_missing_argument() {
    let r = run("awk -v");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "awk: option -v requires an argument\n");
}

#[test]
fn option_v_invalid_assignment() {
    let r = run("awk -v foo 'BEGIN{print 1}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "awk: invalid -v assignment: foo\n");
}

#[test]
fn option_v_attached_form() {
    let r = run("awk -vx=3 'BEGIN{print x}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "3\n");
}

#[test]
fn option_v_attached_invalid_assignment() {
    let r = run("awk -vfoo 'BEGIN{print 1}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "awk: invalid -v assignment: foo\n");
}

#[test]
fn option_progfile_missing_argument() {
    let r = run("awk -f");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "awk: option -f requires an argument\n");
}

#[test]
fn double_dash_makes_remaining_args_files() {
    // Divergence: real awk treats the argument after `--` as the program;
    // this implementation makes everything after `--` an input file, so the
    // program is missing entirely.
    let r = run("awk -- '{print}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "awk: no program text\n");
}

#[test]
fn unknown_option() {
    let r = run("awk -z 'BEGIN{print 1}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "awk: unknown option: -z\n");
}

#[test]
fn progfile_unreadable() {
    let r = run("awk -f /nonexistent");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "awk: can't open source file '/nonexistent': No such file or directory: /nonexistent\n"
    );
}

#[test]
fn empty_program_text() {
    let r = run("awk ''");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "awk: no program text\n");
}

#[test]
fn dash_reads_standard_input() {
    let r = run("printf 'hi\\n' | awk '{print FILENAME, $0}' -");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "(standard input) hi\n");
}

#[test]
fn input_file_unreadable() {
    let r = run("awk '{print}' /nonexistent");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "awk: can't open file '/nonexistent': No such file or directory: /nonexistent\n"
    );
}

// ── lexer.rs ──────────────────────────────────────────────────────────

#[test]
fn newline_tokens_and_line_continuation() {
    // A program spanning multiple lines produces Newline tokens; a backslash
    // immediately before a newline is a line continuation and produces none.
    let r = run("awk 'BEGIN {\n print \\\n\"x\"\n}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "x\n");
}

#[test]
fn trailing_comment_until_eof() {
    let r = run("printf 'hi\\n' | awk '{print} # done'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "hi\n");
}

#[test]
fn bare_ampersand_is_a_lex_error() {
    let r = run("awk '{x = 1 & 2}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "awk: syntax error: unexpected character '&' at position 7\n"
    );
}

#[test]
fn dot_not_followed_by_digit_is_a_lex_error() {
    let r = run("awk '{x = .}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "awk: syntax error: unexpected character '.' at position 5\n"
    );
}

#[test]
fn leading_dot_number() {
    let r = run("awk 'BEGIN{print .5}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0.5\n");
}

#[test]
fn string_escape_sequences() {
    let r = run(r#"awk 'BEGIN{print "a\tb\rc\\d\"e\a\bf\f\vh\/i"}'"#);
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "a\tb\rc\\d\"e\u{7}\u{8}f\u{c}\u{b}h/i\n");
}

#[test]
fn unknown_string_escape_keeps_backslash() {
    // Divergence: gawk strips the backslash of an unknown escape (printing
    // "xqy" with a warning); this implementation keeps `\q` verbatim.
    let r = run(r#"awk 'BEGIN{print "x\qy"}'"#);
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "x\\qy\n");
}

#[test]
fn unterminated_string_literal() {
    let r = run("awk 'BEGIN{print \"abc}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "awk: syntax error: unterminated string literal\n");
}

#[test]
fn unterminated_string_escape() {
    // The awk program ends with a backslash inside a string literal.
    let r = run("awk 'BEGIN{print \"abc\\'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "awk: syntax error: unterminated string escape\n");
}

#[test]
fn hex_number_literal() {
    // gawk supports hexadecimal literals in program text as an extension
    // (also printing 26); POSIX awk/mawk do not recognize them.
    let r = run("awk 'BEGIN{print 0x1A}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "26\n");
}

#[test]
fn hex_number_without_digits() {
    let r = run("awk 'BEGIN{print 0x}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "awk: syntax error: invalid hex number: cannot parse integer from empty string\n"
    );
}

#[test]
fn scientific_notation_numbers() {
    let r = run("awk 'BEGIN{print 1e3, 1e-3, 1.5E+2}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1000 0.001 150\n");
}

#[test]
fn regex_with_escaped_slash() {
    let r = run("printf 'a/b\\n' | awk '/a\\/b/ {print}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "a/b\n");
}

#[test]
fn unterminated_regex_at_newline() {
    // A real newline inside a regex literal is a lex error.
    let r = run("awk '/ab\ncd/ {print}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "awk: syntax error: unterminated regex literal\n");
}

#[test]
fn unterminated_regex_at_eof() {
    let r = run("awk '/abc {print}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "awk: syntax error: unterminated regex literal\n");
}

#[test]
fn regex_after_newline_at_rule_boundary() {
    // After a value token (here `1`) followed by a newline, `/` starts a
    // regex (rule boundary), not a division.
    let r = run("printf 'a\\nfoo\\n' | awk 'NR == 1\n/foo/ { print }'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "a\nfoo\n");
}

// ── parser.rs: token Display in error messages ────────────────────────

#[test]
fn parse_error_messages_display_unexpected_tokens() {
    // Each malformed program fails `expect()` or `parse_primary()` with a
    // different token in the error message, exercising Token's Display impl.
    let cases: &[(&str, &str)] = &[
        ("{while +}", "expected (, got +"),
        ("{while -}", "expected (, got -"),
        ("{while *}", "expected (, got *"),
        ("{x = 1 + / }", "unexpected token in expression: /"),
        ("{a[1 }", "expected ], got }"),
        ("{while %}", "expected (, got %"),
        ("{while ^}", "expected (, got ^"),
        ("{while =}", "expected (, got ="),
        ("{while +=}", "expected (, got +="),
        ("{while -=}", "expected (, got -="),
        ("{while *=}", "expected (, got *="),
        ("{x = 1 + /= 2}", "unexpected token in expression: /="),
        ("{while %=}", "expected (, got %="),
        ("{while ^=}", "expected (, got ^="),
        ("{while ==}", "expected (, got =="),
        ("{while !=}", "expected (, got !="),
        ("{while <}", "expected (, got <"),
        ("{while <=}", "expected (, got <="),
        ("{while >}", "expected (, got >"),
        ("{while >=}", "expected (, got >="),
        ("{while ~}", "expected (, got ~"),
        ("{while !~}", "expected (, got !~"),
        ("{while &&}", "expected (, got &&"),
        ("{while ||}", "expected (, got ||"),
        ("{while !}", "expected (, got !"),
        ("{while ++}", "expected (, got ++"),
        ("{while --}", "expected (, got --"),
        ("{while $}", "expected (, got $"),
        ("{while ()}", "unexpected token in expression: )"),
        ("}", "unexpected token in expression: }"),
        ("{while {}", "expected (, got {"),
        ("{while [}", "expected (, got ["),
        ("{while ;}", "expected (, got ;"),
        ("{while ,}", "expected (, got ,"),
        ("{while ?}", "expected (, got ?"),
        ("{while :}", "expected (, got :"),
        ("{while >>}", "expected (, got >>"),
        ("{while |}", "expected (, got |"),
        ("{while 3}", "expected (, got 3"),
        ("{while \"s\"}", "expected (, got \"s\""),
        ("{while foo}", "expected (, got foo"),
        ("{while /re/}", "expected (, got /re/"),
        ("{while BEGIN}", "expected (, got BEGIN"),
        ("{while END}", "expected (, got END"),
        ("{while if}", "expected (, got if"),
        ("{while else}", "expected (, got else"),
        ("{while while}", "expected (, got while"),
        ("{while for}", "expected (, got for"),
        ("{while do}", "expected (, got do"),
        ("{while break}", "expected (, got break"),
        ("{while continue}", "expected (, got continue"),
        ("{while next}", "expected (, got next"),
        ("{while exit}", "expected (, got exit"),
        ("{while in}", "expected (, got in"),
        ("{while delete}", "expected (, got delete"),
        ("{while getline}", "expected (, got getline"),
        ("{while print}", "expected (, got print"),
        ("{while printf}", "expected (, got printf"),
        // A newline between `if` and `(` is reported via Token::Newline.
        ("{if\n(1) print}", "expected (, got \\n"),
        // `expected` side of the remaining punctuation tokens.
        ("{x = (1 ? 2)}", "expected :, got )"),
        ("BEGIN{", "expected }, got EOF"),
        ("BEGIN{for (i=0 i<3; i++) print i}", "expected ;, got )"),
    ];
    for (prog, msg) in cases {
        let r = run(&format!("awk '{prog}'"));
        assert_eq!(r.exit_code, 2, "prog: {prog:?}");
        assert_eq!(
            r.stderr,
            format!("awk: syntax error: {msg}\n"),
            "prog: {prog:?}"
        );
    }
}

// ── parser.rs: other error paths ──────────────────────────────────────

#[test]
fn do_without_while_is_a_parse_error() {
    let r = run("awk '{do print \"x\"}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "awk: syntax error: expected 'while' after 'do' body\n"
    );
}

#[test]
fn for_in_requires_array_name() {
    let r = run("awk '{for (x in 5) print}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "awk: syntax error: expected array name in for-in\n"
    );
}

#[test]
fn delete_requires_array_name() {
    let r = run("awk '{delete 5}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "awk: syntax error: expected array name after 'delete'\n"
    );
}

#[test]
fn in_requires_array_name() {
    let r = run("awk '{if (1 in 5) print}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "awk: syntax error: expected array name after 'in'\n"
    );
}

// ── parser.rs: features ───────────────────────────────────────────────

#[test]
fn pattern_and_action_on_separate_lines() {
    let r = run("printf 'err\\nok\\n' | awk '/err/\n{print}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "err\n");
}

#[test]
fn expression_range_pattern() {
    let r = run("printf '1\\n2\\n3\\n4\\n' | awk 'NR==2, NR==3 {print}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "2\n3\n");
}

#[test]
fn print_output_redirection_is_parsed_but_ignored() {
    // Divergence (the lexer documents redirection as "parsed but not fully
    // supported"): `> "file"` after a print expression parses as a comparison
    // ("x" > "/f" is true, printing 1); `>>` and `|` hit the redirect-skip
    // path. Either way, output still goes to stdout.
    let r = run("awk 'BEGIN{print \"x\" > \"/f\"; print \"y\" >> \"/f\"; print \"z\" | \"cat\"}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1\ny\nz\n");
}

#[test]
fn c_style_for_with_empty_clauses() {
    let r = run("awk 'BEGIN{i=0; for(;;){i++; if(i==3) break}; print i}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "3\n");
}

#[test]
fn delete_multi_dimensional_index() {
    let r = run("awk 'BEGIN{a[1,2]=\"x\"; delete a[1,2]; print length(a)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0\n");
}

#[test]
fn compound_assignment_operators() {
    let r = run("awk 'BEGIN{x=8; x/=2; print x; x%=3; print x; x^=3; print x}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "4\n1\n1\n");
}

#[test]
fn greater_equal_comparison() {
    let r = run("awk 'BEGIN{print (3 >= 2), (1 >= 2)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1 0\n");
}

#[test]
fn unary_plus() {
    let r = run("awk 'BEGIN{print +5, +\"3\"}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "5 3\n");
}

#[test]
fn pre_decrement() {
    let r = run("awk 'BEGIN{x=5; print --x}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "4\n");
}

#[test]
fn parenthesized_variable_as_array_ref() {
    // Divergence: gawk rejects `(a)[1]` as a syntax error; this
    // implementation's postfix subscript accepts it.
    let r = run("awk 'BEGIN{a[1]=\"x\"; print (a)[1]}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "x\n");
}

#[test]
fn subscript_after_non_variable_is_a_parse_error() {
    let r = run("awk '{print (1)[1]}'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "awk: syntax error: unexpected token in expression: [\n"
    );
}

#[test]
fn getline_stub_returns_zero() {
    // getline is a documented stub ("not fully supported") that yields 0.
    let r = run("awk 'BEGIN{print (getline)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0\n");
}

// ── runtime.rs: values ────────────────────────────────────────────────

#[test]
fn string_and_uninitialized_truthiness() {
    let r = run("printf 'abc\\n' | awk '{if ($0) print \"t\"}'");
    assert_eq!(r.stdout, "t\n");
    let r = run("awk 'BEGIN{if (x) print \"t\"; else print \"f\"}'");
    assert_eq!(r.stdout, "f\n");
}

#[test]
fn inf_and_nan_formatting() {
    // Divergence (minor): gawk warns and prints "-nan" for sqrt(-1); this
    // implementation prints "nan" with no warning and continues (exit 0).
    let r = run("awk 'BEGIN{print 1e999, -1e999, sqrt(-1)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "inf -inf nan\n");
}

#[test]
fn string_to_number_parsing() {
    let r = run(
        "awk 'BEGIN{print \"   \" + 0, \".5\" + 0, \"1e\" + 0, \"2.5e-1\" + 0, \"1e5x\" + 0, \"abc\" + 0}'",
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0 0.5 1 0.25 100000 0\n");
}

#[test]
fn v_assignment_with_non_finite_number_stays_string() {
    let r = run("awk -v x=1e999 'BEGIN{print x}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1e999\n");
}

// ── runtime.rs: execution flow ────────────────────────────────────────

#[test]
fn exit_in_begin_skips_end_rules() {
    // Matches gawk, which also skips END rules after `exit` in BEGIN.
    let r = run("awk 'BEGIN{exit 3} END{print \"end\"}'");
    assert_eq!(r.exit_code, 3);
    assert_eq!(r.stdout, "");
}

#[test]
fn top_level_break_is_silently_ignored() {
    // Divergence: gawk aborts with a fatal error for `break` outside a
    // loop; this implementation aborts the action and continues with the
    // next record.
    let r = run("printf 'x\\n' | awk '{break; print}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "");
}

#[test]
fn exit_in_end_sets_exit_code() {
    let r = run("printf 'a\\n' | awk '{print} END{exit 5} END{print \"no\"}'");
    assert_eq!(r.exit_code, 5);
    assert_eq!(r.stdout, "a\n");
}

#[test]
fn range_pattern_reactivates() {
    let r = run("printf 'a\\nb\\nc\\nb\\nc\\nd\\n' | awk '/b/,/c/ {print}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "b\nc\nb\nc\n");
}

#[test]
fn assigning_nf_zero_clears_record() {
    let r = run("printf 'a b c\\n' | awk '{NF=0; print \"[\" $0 \"]\"}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "[]\n");
}

#[test]
fn assigning_nf_extends_record() {
    let r = run("printf 'a b\\n' | awk '{NF=4; print \"[\" $0 \"]\", NF}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "[a b  ] 4\n");
}

#[test]
fn out_of_range_field_is_empty() {
    let r = run("printf 'a b\\n' | awk '{print \"[\" $5 \"]\"}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "[]\n");
}

#[test]
fn field_index_limit() {
    let r = run("printf 'a b\\n' | awk '{$10001 = \"x\"; print NF}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "2\n");
    assert_eq!(r.stderr, "awk: field index 10001 exceeds limit 10000\n");
}

#[test]
fn field_assignment_extends_record() {
    let r = run("printf 'a b\\n' | awk '{$5 = \"x\"; print $0; print NF}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "a b   x\n5\n");
}

// ── runtime.rs: loop control flow and limits ──────────────────────────

#[test]
fn while_loop_signals() {
    let r = run("awk 'BEGIN{while(1){break}; print \"ok\"}'");
    assert_eq!(r.stdout, "ok\n");

    let r = run("awk 'BEGIN{i=0; while(i<3){i++; continue; i=100}; print i}'");
    assert_eq!(r.stdout, "3\n");

    // `next` inside a while loop skips the rest of the rules for the record.
    let r = run("printf 'a\\nb\\n' | awk '{while(1){next}; print}'");
    assert_eq!(r.stdout, "");

    let r = run("awk 'BEGIN{while(1){exit 2}}'");
    assert_eq!(r.exit_code, 2);
}

#[test]
fn do_while_loop_signals() {
    let r = run("awk 'BEGIN{do {break} while(1); print \"ok\"}'");
    assert_eq!(r.stdout, "ok\n");

    let r = run("awk 'BEGIN{i=0; do {i++; continue; i=100} while(i<3); print i}'");
    assert_eq!(r.stdout, "3\n");

    let r = run("printf 'a\\n' | awk '{do {next} while(0); print}'");
    assert_eq!(r.stdout, "");

    let r = run("awk 'BEGIN{do {exit 2} while(1)}'");
    assert_eq!(r.exit_code, 2);
}

#[test]
fn for_loop_signals() {
    // An `exit` in the init clause propagates out of the loop.
    let r = run("awk 'BEGIN{for(exit 7; 0; 0) print \"x\"}'");
    assert_eq!(r.exit_code, 7);
    assert_eq!(r.stdout, "");

    let r = run("awk 'BEGIN{for(i=0;i<3;i++){continue; i=100}; print i}'");
    assert_eq!(r.stdout, "3\n");

    let r = run("printf 'a\\nb\\n' | awk '{for(i=0;i<1;i++) next; print}'");
    assert_eq!(r.stdout, "");

    let r = run("awk 'BEGIN{for(i=0;i<3;i++) exit 4}'");
    assert_eq!(r.exit_code, 4);
}

#[test]
fn for_in_loop_signals() {
    let r = run("awk 'BEGIN{a[1]=1; for(k in a){break}; print \"ok\"}'");
    assert_eq!(r.stdout, "ok\n");

    let r = run("awk 'BEGIN{a[1]=1; a[2]=2; c=0; for(k in a){c++; continue; c=100}; print c}'");
    assert_eq!(r.stdout, "2\n");

    let r = run("printf 'x\\ny\\n' | awk '{a[1]=1; for(k in a) next; print}'");
    assert_eq!(r.stdout, "");

    let r = run("awk 'BEGIN{a[1]=1; for(k in a) exit 6}'");
    assert_eq!(r.exit_code, 6);
}

#[test]
fn while_loop_iteration_limit() {
    let limits = ExecutionLimits {
        max_loop_iterations: 100,
        ..Default::default()
    };
    let r = run_with_limits("awk 'BEGIN{while(1) i++}'", limits);
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stderr, "awk: loop iteration limit exceeded\n");
}

#[test]
fn do_while_loop_iteration_limit() {
    let limits = ExecutionLimits {
        max_loop_iterations: 100,
        ..Default::default()
    };
    let r = run_with_limits("awk 'BEGIN{do i++ while(1)}'", limits);
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stderr, "awk: loop iteration limit exceeded\n");
}

#[test]
fn for_loop_iteration_limit() {
    let limits = ExecutionLimits {
        max_loop_iterations: 100,
        ..Default::default()
    };
    let r = run_with_limits("awk 'BEGIN{for(;;) i++}'", limits);
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stderr, "awk: loop iteration limit exceeded\n");
}

#[test]
fn for_in_loop_iteration_limit() {
    let limits = ExecutionLimits {
        max_loop_iterations: 100,
        ..Default::default()
    };
    // Build a 200-element array via split (no loop), then iterate it.
    let words: Vec<String> = (0..200).map(|i| format!("w{i}")).collect();
    let script = format!(
        "awk 'BEGIN{{ n=split(\"{}\", a); for(k in a) j++ }}'",
        words.join(" ")
    );
    let r = run_with_limits(&script, limits);
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stderr, "awk: loop iteration limit exceeded\n");
}

#[test]
fn print_output_size_limit() {
    let limits = ExecutionLimits {
        max_output_size: 100,
        ..Default::default()
    };
    // awk's internal guard stops output growth once stdout exceeds the
    // limit; the interpreter then reports the exceeded limit as an error.
    let err = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap()
        .exec("awk 'BEGIN{for(i=0;i<50;i++) print \"abcdef\"}'")
        .unwrap_err();
    assert!(
        format!("{err:?}").contains("max_output_size"),
        "err: {err:?}"
    );
}

#[test]
fn printf_output_size_limit() {
    let limits = ExecutionLimits {
        max_output_size: 100,
        ..Default::default()
    };
    // Same as print: awk's guard stops growth, the interpreter reports it.
    let err = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap()
        .exec("awk 'BEGIN{for(i=0;i<50;i++) printf \"abcdef\\n\"}'")
        .unwrap_err();
    assert!(
        format!("{err:?}").contains("max_output_size"),
        "err: {err:?}"
    );
}

// ── runtime.rs: operators ─────────────────────────────────────────────

#[test]
fn match_operator_with_dynamic_pattern() {
    let r =
        run("printf 'abc\\n' | awk '{if ($0 ~ \"^a\") print \"m\"; if ($0 !~ \"z\") print \"n\"}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "m\nn\n");
}

#[test]
fn division_and_modulo_by_zero() {
    // Divergence: gawk aborts with a fatal error on division by zero; this
    // implementation warns on stderr and yields 0.
    let r = run("printf 'x\\n' | awk '{print 1/0; print 1%0}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0\n0\n");
    assert_eq!(r.stderr, "awk: division by zero\nawk: division by zero\n");
}

#[test]
fn compound_assignment_by_zero_and_pow() {
    let r = run(
        "awk 'BEGIN{x=10; x/=0; print x; x=10; x/=4; print x; x%=0; print x; x=10; x%=3; print x; x=2; x^=3; print x}'",
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0\n2.5\n0\n1\n8\n");
    assert_eq!(r.stderr, "awk: division by zero\nawk: division by zero\n");
}

#[test]
fn assignment_to_non_lvalue_is_silently_ignored() {
    // Divergence: gawk rejects `1 = 2` at parse time; this implementation
    // evaluates and silently discards the assignment.
    let r = run("awk 'BEGIN{1 = 2; print \"ok\"}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn string_comparisons() {
    let r = run(
        "awk 'BEGIN{print (\"b\" > \"a\"), (\"a\" < \"b\"), (\"a\" <= \"a\"), (\"b\" >= \"b\"), (\"a\" == \"a\"), (\"a\" != \"b\"), (\"a\" > \"b\")}'",
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1 1 1 1 1 1 0\n");
}

#[test]
fn uninitialized_variable_compares_numerically() {
    let r = run("printf 'x\\n' | awk '{print (y == 0)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn empty_string_is_not_numeric_in_comparison() {
    let r = run("awk 'BEGIN{print (\"\" == 0), (\"  \" == 0)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0 0\n");
}

// ── runtime.rs: built-in functions ────────────────────────────────────

#[test]
fn length_with_empty_parens() {
    let r = run("printf 'hello\\n' | awk '{print length()}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "5\n");
}

#[test]
fn substr_edge_cases() {
    let r = run(
        "awk 'BEGIN{print \"[\" substr(\"x\") \"]\", \"[\" substr(\"abc\", 10) \"]\", \"[\" substr(\"hello\", 0, 3) \"]\"}'",
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "[] [] [he]\n");
}

#[test]
fn index_edge_cases() {
    // Divergence (suspected): gawk returns 1 for an empty needle; this
    // implementation returns 0.
    let r = run("awk 'BEGIN{print index(\"x\"), index(\"abc\", \"\")}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0 0\n");
}

#[test]
fn split_edge_cases() {
    let r = run("awk 'BEGIN{print split(\"a:b\"), split(\"a:b\", \"arr\")}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0 0\n");

    // Without a separator argument, split uses FS.
    let r = run("awk 'BEGIN{n=split(\"a b c\", arr); print n, arr[2]}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "3 b\n");
}

#[test]
fn match_edge_cases() {
    // A string (not /regex/) pattern is compiled at runtime.
    let r = run("awk 'BEGIN{print match(\"x\"), match(\"abc123\", \"[0-9]+\"), RSTART, RLENGTH}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0 4 4 3\n");

    let r = run("awk 'BEGIN{print match(\"x\", \"[\")}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0\n");
    assert!(
        r.stderr.starts_with("awk: invalid regex '[':"),
        "stderr: {:?}",
        r.stderr
    );
}

#[test]
fn builtins_called_with_no_arguments() {
    let r = run(
        "awk 'BEGIN{print \"[\" sprintf() \"]\", \"[\" tolower() \"]\", \"[\" toupper() \"]\", int(), sqrt()}'",
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "[] [] [] 0 0\n");
}

#[test]
fn math_functions() {
    let r = run(
        "awk 'BEGIN{print sin(0), cos(0), atan2(0,1), atan2(1), exp(0), log(1), sin(), cos(), exp(), log()}'",
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0 1 0 0 1 0 0 1 1 -inf\n");
}

#[test]
fn rand_is_deterministic_after_srand() {
    let r = run("awk 'BEGIN{srand(7); a=rand(); srand(7); b=rand(); print (a==b), (a>=0 && a<1)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1 1\n");
}

#[test]
fn srand_returns_previous_seed() {
    let r = run("awk 'BEGIN{srand(42); print srand(1)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "42\n");
}

#[test]
fn srand_without_argument_seeds_from_time() {
    // srand() returns the previous seed (0 on a fresh runtime); the new
    // time-derived seed must still produce an in-range rand().
    let r = run("awk 'BEGIN{print srand(), (rand() >= 0 && rand() < 1)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0 1\n");
}

#[test]
fn unknown_function_warns_and_continues() {
    // Divergence: gawk rejects unknown functions at parse time; this
    // implementation warns at runtime and yields the empty value.
    let r = run("awk 'BEGIN{print \"[\" foo(1) \"]\"}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "[]\n");
    assert_eq!(r.stderr, "awk: unknown function 'foo'\n");
}

#[test]
fn sub_edge_cases() {
    let r = run("awk 'BEGIN{print sub(/x/)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0\n");

    // A string (not /regex/) pattern is compiled at runtime.
    let r = run("printf 'hello world\\n' | awk '{sub(\"world\", \"earth\"); print}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "hello earth\n");

    // An explicit target is substituted in place.
    let r = run("printf 'a b\\n' | awk '{n = sub(/b/, \"B\", $2); print n, $0}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1 a B\n");

    let r = run("awk 'BEGIN{s=\"x\"; print sub(\"[\", \"y\", s), s}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0 x\n");
    assert!(
        r.stderr.starts_with("awk: invalid regex '[':"),
        "stderr: {:?}",
        r.stderr
    );
}

#[test]
fn sub_replacement_escapes() {
    // & in the replacement expands to the matched text.
    let r = run("awk 'BEGIN{s=\"foo\"; gsub(/o/, \"[&]\", s); print s}'");
    assert_eq!(r.stdout, "f[o][o]\n");

    // \& is a literal ampersand.
    let r = run("awk 'BEGIN{s=\"ab\"; sub(/b/, \"\\\\&\", s); print s}'");
    assert_eq!(r.stdout, "a&\n");

    // \\ is a literal backslash.
    let r = run("awk 'BEGIN{s=\"ab\"; sub(/b/, \"\\\\\\\\\", s); print s}'");
    assert_eq!(r.stdout, "a\\\n");

    // Any other escaped character loses the backslash.
    let r = run("awk 'BEGIN{s=\"ab\"; sub(/b/, \"\\\\q\", s); print s}'");
    assert_eq!(r.stdout, "aq\n");
}

// ── runtime.rs: regex handling ────────────────────────────────────────

#[test]
fn invalid_regex_in_pattern() {
    let r = run("printf 'a\\n' | awk '/[/ {print}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "");
    assert!(
        r.stderr.starts_with("awk: invalid regex '[':"),
        "stderr: {:?}",
        r.stderr
    );
}

#[test]
fn regex_cache_overflow_clears_cache() {
    // More than 1000 distinct dynamic patterns trigger the cache-clear guard.
    let r = run("awk 'BEGIN{c=0; for(i=0;i<1100;i++){if ((i \"\") ~ (\"^\" i)) c++}; print c}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1100\n");
}

// ── runtime.rs: field and record splitting ────────────────────────────

#[test]
fn multi_character_fs_is_a_regex() {
    let r = run("printf 'a:  b: c\\n' | awk -F': *' '{print $2, $3, NF}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "b c 3\n");
}

#[test]
fn invalid_regex_fs_yields_single_field() {
    let r = run("printf 'xa[y\\n' | awk -v FS='a[' '{print NF, $1}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1 xa[y\n");
}

#[test]
fn paragraph_mode() {
    let r = run("printf 'a\\nb\\n\\nc\\nd\\n' | awk -v RS= '{print NR, NF}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1 2\n2 2\n");

    // Consecutive blank lines are skipped, and a final paragraph without a
    // trailing newline is still a record.
    let r = run("printf 'a\\n\\n\\nb' | awk -v RS= '{print NR, $0}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1 a\n2 b\n");
}

#[test]
fn single_character_rs() {
    let r = run("printf 'a:b:' | awk -v RS=':' '{print NR, $0}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1 a\n2 b\n");
}

#[test]
fn regex_rs() {
    let r = run("printf 'aXXbXX' | awk -v RS='X+' '{print NR, $0}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1 a\n2 b\n");
}

#[test]
fn invalid_regex_rs_yields_single_record() {
    let r = run("printf 'a\\nb\\n' | awk -v RS='a[' '{print NR}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn dot_at_end_of_program_is_a_lex_error() {
    // A `.` at the very end of the input cannot start a number.
    let r = run("awk '{print 1}.'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "awk: syntax error: unexpected character '.' at position 9\n"
    );
}

#[test]
fn exit_without_code() {
    let r = run("printf 'x\\n' | awk '{print} END{exit}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "x\n");
}

#[test]
fn parenthesized_variable_multi_dimensional_subscript() {
    let r = run("awk 'BEGIN{a[1,2]=\"x\"; print (a)[1,2]}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "x\n");
}

#[test]
fn range_pattern_start_and_end_on_same_record() {
    let r = run("printf 'a\\nb\\nc\\n' | awk '/b/,/b/ {print}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "b\n");
}

#[test]
fn sprintf_scientific_with_precision() {
    let r = run("awk 'BEGIN{print sprintf(\"%.2e %.2E %.2g\", 12345.678, 12345.678, 12345.678)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1.23e+04 1.23E+04 1.2e+04\n");
}

// ── runtime.rs: sprintf ───────────────────────────────────────────────

#[test]
fn sprintf_percent_octal_hex_and_unknown_specifier() {
    let r = run(
        "awk 'BEGIN{print sprintf(\"100%%\"), sprintf(\"%o %x %X\", 8, 255, 255), sprintf(\"%q\", 1)}'",
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "100% 10 ff FF %q\n");
}

#[test]
fn sprintf_float_and_precision() {
    let r = run("awk 'BEGIN{print sprintf(\"%f\", 1.5), sprintf(\"%.2f\", 3.14159)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1.500000 3.14\n");
}

#[test]
fn sprintf_scientific() {
    let r = run(
        "awk 'BEGIN{print sprintf(\"%e\", 12345.678), sprintf(\"%E\", 12345.678), sprintf(\"%e\", 0)}'",
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1.234568e+04 1.234568E+04 0.000000e+00\n");
}

#[test]
fn sprintf_g_format() {
    // Divergence (suspected): C/gawk `%g` strips trailing zeros in
    // scientific notation ("1.2345e-05"); this implementation keeps them.
    let r = run(
        "awk 'BEGIN{print sprintf(\"%g\", 0), sprintf(\"%g\", 2.5), sprintf(\"%g\", 100000), sprintf(\"%g\", 1), sprintf(\"%g\", 123456789), sprintf(\"%g\", 0.000012345)}'",
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "0 2.5 100000 1 1.23457e+08 1.23450e-05\n");
}

#[test]
fn sprintf_star_width_and_precision() {
    let r = run(
        "awk 'BEGIN{print sprintf(\"%*d\", 5, 42), sprintf(\"%.*f\", 2, 3.14159), sprintf(\"[%*d]\", 3)}'",
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "   42 3.14 [  0]\n");
}

#[test]
fn sprintf_incomplete_format_specifiers() {
    let r = run("awk 'BEGIN{print \"[\" sprintf(\"abc%\") \"]\", \"[\" sprintf(\"%5\") \"]\"}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "[abc%] []\n");
}

#[test]
fn sprintf_missing_arguments() {
    let r = run("awk 'BEGIN{print sprintf(\"%d %s\", 1)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "1 \n");
}

#[test]
fn sprintf_string_precision_and_char() {
    let r = run(
        "awk 'BEGIN{print sprintf(\"%.2s\", \"hello\"), sprintf(\"%c\", \"abc\"), sprintf(\"%c\", 65), \"[\" sprintf(\"%c\", 1114112) \"]\"}'",
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "he a A []\n");
}

#[test]
fn sprintf_zero_padding_and_plus_flag() {
    // Divergence: the `+` flag is accepted but ignored (gawk prints "+5").
    let r = run("awk 'BEGIN{print sprintf(\"%05d\", -42), sprintf(\"%+d\", 5)}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "-0042 5\n");
}

#[test]
fn sprintf_backslash_escapes() {
    // The awk source `\\n` yields a literal backslash-n at runtime, which
    // awk_sprintf then converts to a newline.
    let r = run("awk 'BEGIN{printf \"a\\\\nb\\\\n\"}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "a\nb\n");

    let r = run("awk 'BEGIN{printf \"\\\\t\\\\r\\\\\\\\\\\\a\\\\b\\\\f\\\\/\\\\q\\\\n\"}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "\t\r\\\u{7}\u{8}\u{c}/\\q\n");

    // \" escape inside the format string.
    let r = run("awk 'BEGIN{printf \"\\\\\\\"\"}'");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "\"");

    // A trailing backslash is kept verbatim.
    let r = run(r#"awk 'BEGIN{printf "abc\\"}'"#);
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "abc\\");
}
