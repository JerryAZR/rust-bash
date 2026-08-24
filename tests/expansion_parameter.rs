//! Coverage tests for src/interpreter/expansion.rs: parameter expansion
//! operators — defaults, assignment defaults, error/alternative operators,
//! substring slicing, pattern strip/replace, case modification, and indirect
//! expansion.
//!
//! Several operators exist in both a mutable and an immutable code path. The
//! immutable path is only reachable when the expansion appears inside a
//! pattern or replacement string of another expansion (e.g. `${v%${y:-d}}`);
//! those tests live in the "immutable path via pattern context" section.
//!
//! Where the pinned behavior is suspected to diverge from real bash, the
//! expectation carries a `DIVERGENCE?` comment.

use rust_bash::{RustBash, RustBashBuilder};

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

// ── Immutable path via pattern context: default-value operators ─────

#[test]
fn pattern_default_uses_value_when_variable_set() {
    let (out, err, code) = run(r#"y=c; v=abc; echo "${v%${y:-zz}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

#[test]
fn pattern_default_uses_default_when_variable_unset() {
    let (out, err, code) = run(r#"v=abc; echo "${v%${y:-c}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

#[test]
fn pattern_assign_default_expands_default_in_immutable_context() {
    // DIVERGENCE?: real bash also assigns y=c here; the immutable pattern
    // expansion path only evaluates the default without assigning.
    let mut sh = shell();
    let r = sh
        .exec(r#"v=abc; echo "${v%${y:=c}}"; echo "[${y:-unset}]""#)
        .unwrap();
    assert_eq!(r.stdout, "ab\n[unset]\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn pattern_assign_default_uses_value_when_variable_set() {
    let (out, err, code) = run(r#"y=c; v=abc; echo "${v%${y:=zz}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

#[test]
fn pattern_error_operator_aborts_when_variable_unset() {
    let (out, err, code) = run(r#"v=abc; echo "${v%${y:?boom}}""#);
    assert_eq!(out, "");
    assert_eq!(err, "rust-bash: y: boom\n");
    assert_eq!(code, 1);
}

#[test]
fn pattern_error_operator_uses_value_when_variable_set() {
    let (out, err, code) = run(r#"y=c; v=abc; echo "${v%${y:?boom}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

#[test]
fn pattern_alternative_expands_when_variable_set() {
    let (out, err, code) = run(r#"y=1; v=abc; echo "${v%${y:+c}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

#[test]
fn pattern_alternative_expands_to_nothing_when_variable_unset() {
    let (out, err, code) = run(r#"v=abc; echo "${v%${y:+c}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("abc\n", "", 0));
}

#[test]
fn pattern_alternative_with_empty_array_marks_at_empty() {
    let (out, err, code) = run(r#"arr=(); v=abc; echo "${v%${arr[@]:+c}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("abc\n", "", 0));
}

// ── Immutable path via pattern context: substring ───────────────────

#[test]
fn replacement_scalar_substring_variants_in_immutable_context() {
    // Double-quoted replacement strings are expanded through the immutable
    // word path, exercising scalar substring there.
    let (out, err, code) = run(
        r#"s=abcde; v=axb; echo "${v/x/"${s:1:3}"}"; echo "${v/x/"${s: -2}"}"; echo "${v/x/"${s:1:-1}"}""#,
    );
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("abcdb\nadeb\nabcdb\n", "", 0)
    );
}

#[test]
fn replacement_scalar_substring_offset_beyond_length_is_empty() {
    let (out, err, code) = run(r#"s=abc; v=axb; echo "${v/x/"${s:9}"}"; echo "${v/x/"${s:9:2}"}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\nab\n", "", 0));
}

#[test]
fn replacement_positional_slice_in_immutable_context() {
    let (out, err, code) =
        run(r#"set -- p q r; v=axb; echo "${v/x/"${@:1:2}"}"; echo "${v/x/"${@:2}"}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("ap qb\naq rb\n", "", 0)
    );
}

#[test]
fn replacement_array_slice_in_immutable_context() {
    let (out, err, code) = run(r#"a=(p q r); v=axb; echo "${v/x/"${a[@]:1:2}"}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("aq rb\n", "", 0));
}

#[test]
fn replacement_array_slice_negative_length_is_an_error() {
    let (out, err, code) = run(r#"a=(p q r); v=axb; echo "${v/x/"${a[@]:1:-2}"}""#);
    assert_eq!(out, "");
    assert_eq!(err, "rust-bash: -2: substring expression < 0\n");
    assert_eq!(code, 1);
}

// ── Immutable path via pattern context: misc pieces ─────────────────

#[test]
fn bad_ksh_style_word_in_pattern_context_is_rejected() {
    // `[[ x == ${...;} ]]` expands the right-hand word through the immutable
    // pattern path, whose validation rejects the malformed `${...;}` text.
    let (out, err, code) = run(r#"[[ ab == ${myfunc;} ]]"#);
    assert_eq!(out, "");
    assert_eq!(err, "ERR:expansion error: ${myfunc;}: bad substitution");
    assert_eq!(code, -1);
}

#[test]
fn bad_ksh_style_word_in_assignment_is_rejected() {
    let (out, err, code) = run(r#"x=${myfunc;}; echo after"#);
    assert_eq!(out, "");
    assert_eq!(err, "rust-bash: ${myfunc;}: bad substitution\n");
    assert_eq!(code, 1);
}

#[test]
fn pattern_command_substitution_is_not_executed_in_immutable_context() {
    // DIVERGENCE?: real bash executes the command substitution inside the
    // pattern (strip "b" → "ac"); the immutable pattern path drops it, leaving
    // an empty pattern that strips nothing.
    let (out, err, code) = run(r#"v=abc; echo "${v%$(echo b)}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("abc\n", "", 0));
}

#[test]
fn pattern_double_quoted_at_with_no_positionals_produces_empty_pattern() {
    let (out, err, code) = run(r#"v=ab; echo "${v%"$@"}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

#[test]
fn pattern_empty_double_quotes_produce_empty_pattern() {
    let (out, err, code) = run(r#"v=ab; echo "${v%""}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

#[test]
fn gettext_double_quoted_string_expands_like_double_quotes() {
    let (out, err, code) = run(r#"v=world; echo $"hello $v""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("hello world\n", "", 0));
}

// ── Vectorized pattern-strip and case-modification operators ────────

#[test]
fn vectorized_longest_suffix_strip_over_all_elements() {
    let (out, err, code) = run(r#"a=(axb cxb); echo "${a[@]%%x*}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("a c\n", "", 0));
}

#[test]
fn vectorized_longest_prefix_strip_over_all_elements() {
    let (out, err, code) = run(r#"a=(xab xcd); echo "${a[@]##*x}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab cd\n", "", 0));
}

#[test]
fn scalar_strip_with_no_pattern_or_no_match_keeps_value() {
    let (out, err, code) = run(r#"v=abc; echo "${v%}|${v#}|${v%%}|${v##}|${v%z}|${v#z}|${v##z}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("abc|abc|abc|abc|abc|abc|abc\n", "", 0)
    );
}

#[test]
fn vectorized_case_modification_over_all_elements() {
    let (out, err, code) =
        run(r#"a=(ab cd); A=(AB CD); echo "${a[@]^} ${a[@]^^}"; echo "${A[@],} ${A[@],,}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("Ab Cd AB CD\naB cD ab cd\n", "", 0)
    );
}

#[test]
fn case_modification_with_pattern_only_changes_matching_chars() {
    let (out, err, code) = run(r#"x=abc; echo "${x^^[ab]}|${x^[b]}|${x,[b]}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ABc|abc|abc\n", "", 0));
}

// ── ${!prefix*} / ${!prefix@} variable-name expansion ───────────────

#[test]
fn variable_names_star_joins_with_space_when_ifs_unset() {
    let (out, err, code) = run(r#"unset IFS; zz1=a zz2=b; echo "${!zz*}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("zz1 zz2\n", "", 0));
}

#[test]
fn variable_names_at_with_no_matches_expands_to_nothing() {
    let (out, err, code) = run(r#"echo "[${!nomatch@}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[]\n", "", 0));
}

// ── ${!arr[*]} keys with unset IFS ──────────────────────────────────

#[test]
fn array_keys_star_joins_with_space_when_ifs_unset() {
    let (out, err, code) = run(r#"unset IFS; a=(x y); echo "${!a[*]}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("0 1\n", "", 0));
}

#[test]
fn array_keys_of_undefined_and_scalar_variables() {
    let (out, err, code) = run(r#"s=v; echo "${!s[@]}|${s[*]}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("0|v\n", "", 0));
}

#[test]
fn array_keys_and_values_of_empty_scalar_are_empty() {
    let (out, err, code) = run(r#"s=; echo "[${!s[@]}]|[${#s[@]}]|[${s[*]}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[]|[0]|[]\n", "", 0));
}

#[test]
fn array_keys_of_undefined_array_are_empty() {
    let (out, err, code) = run(r#"echo "[${!nope[@]}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[]\n", "", 0));
}

// ── ${#@} / ${#*} positional count ──────────────────────────────────

#[test]
fn length_of_at_and_star_is_positional_count() {
    let (out, err, code) = run(r#"set -- a b c; echo "${#@} ${#*}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("3 3\n", "", 0));
}

// ── Indirect expansion ──────────────────────────────────────────────

#[test]
fn indirect_expansion_of_nameref_returns_target_name() {
    // Bash inverts ${!ref} when ref is itself a nameref: it yields the target
    // name rather than the target's value.
    let (out, err, code) = run(r#"declare -n nr=t; t=v; echo "${!nr}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("t\n", "", 0));
}

#[test]
fn indirect_expansion_targets() {
    let (out, err, code) = run(
        r#"set -- a b; r0=0; r1=1; rat=@; rstar='*'; rhash='#'; rbang='!'; rarr='a[1]'; a=(p q)
echo "${!r0}|${!r1}|${!rat}|${!rstar}|${!rhash}|[${!rbang}]|${!rarr}""#,
    );
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("rust-bash|a|a b|a b|2|[]|q\n", "", 0)
    );
}

#[test]
fn indirect_expansion_star_joins_with_space_when_ifs_unset() {
    let (out, err, code) = run(r#"unset IFS; r='*'; set -- a b; echo "${!r}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("a b\n", "", 0));
}

#[test]
fn indirect_default_with_scalar_target() {
    let (out, err, code) = run(r#"r=x; x=; echo "${!r:-d}|${!r-d}""#);
    // x is set but empty: `:-` uses the default, `-` does not.
    assert_eq!((out.as_str(), err.as_str(), code), ("d|\n", "", 0));
}

#[test]
fn indirect_default_with_empty_target_uses_default() {
    let (out, err, code) = run(r#"r=; echo "${!r:-d}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("d\n", "", 0));
}

#[test]
fn indirect_default_with_positional_targets() {
    let (out, err, code) = run(r#"r=@; set --; echo "${!r:-d}"; set -- a; echo "${!r:-d}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("d\na\n", "", 0));
}

#[test]
fn pattern_indirect_default_via_immutable_context() {
    let (out, err, code) = run(r#"r=z; v=abc; echo "${v%${!r:-c}}"; r=; echo "${v%${!r:-c}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\nab\n", "", 0));
}

#[test]
fn pattern_vectorized_default_via_immutable_context() {
    // DIVERGENCE?: in pattern context, `${a[@]:-y}` (with an empty array)
    // expands to an empty pattern instead of the default "y" that the
    // mutable path produces, so nothing is stripped. The `-` form agrees
    // with bash (an empty array is set, so the default does not apply).
    let (out, err, code) = run(r#"a=(); v=xyz; echo "${v%${a[@]:-y}}|${v%${a[@]-y}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("xyz|xyz\n", "", 0));
}

// ── Array/scalar element resolution ─────────────────────────────────

#[test]
fn negative_array_index_counts_from_the_end() {
    // DIVERGENCE?: real bash rejects negative array subscripts entirely; here
    // ${a[-1]} counts from the end and an out-of-range negative index warns
    // on stderr but still expands empty with a success exit code.
    let (out, err, code) = run(r#"a=(x y z); echo "${a[-1]}|[${a[-9]}]""#);
    assert_eq!(out, "z|[]\n");
    assert_eq!(err, "rust-bash: line 1: a: bad array subscript\n");
    assert_eq!(code, 0);
}

#[test]
fn negative_array_index_in_pattern_context() {
    // Pattern strings expand through the immutable path; negative indices
    // count from the end there as well, silently empty when out of range.
    let (out, err, code) = run(r#"a=(x y); v=xy; echo "${v%${a[-1]}}|[${v%${a[-9]}}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("x|[xy]\n", "", 0));
}

#[test]
fn scalar_subscript_beyond_zero_is_empty() {
    let (out, err, code) = run(r#"s=hi; echo "[${s[2]}]|[${s[5]}]|${s[0]}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[]|[]|hi\n", "", 0));
}

#[test]
fn nameref_to_empty_array_subscript_resolves_empty() {
    let (out, err, code) = run(r#"a=(x); declare -n r='a[]'; echo "[$r]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[]\n", "", 0));
}

#[test]
fn funcname_element_in_pattern_context() {
    let (out, err, code) = run(r#"f() { v=abc; echo "${v%${FUNCNAME[0]}}"; }; f"#);
    assert_eq!((out.as_str(), err.as_str(), code), ("abc\n", "", 0));
}

#[test]
fn funcname_negative_index_and_out_of_bounds() {
    let (out, err, code) = run(r#"f() { echo "${FUNCNAME[-1]}|[${FUNCNAME[-9]}]"; }; f"#);
    assert_eq!((out.as_str(), err.as_str(), code), ("f|[]\n", "", 0));
}

#[test]
fn funcname_star_joining_and_lineno_values() {
    let (out, err, code) =
        run(r#"g() { IFS=,; echo "${FUNCNAME[*]}"; echo "${BASH_LINENO[@]}"; }; f() { g; }; f"#);
    assert_eq!((out.as_str(), err.as_str(), code), ("g,f\n1 1\n", "", 0));
}

#[test]
fn array_slice_variants() {
    let (out, err, code) =
        run(r#"f() { echo "${FUNCNAME[@]:0:1}"; }; f; s=v; echo "[${s[@]:0:1}]|[${nope[@]:0:1}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("f\n[v]|[]\n", "", 0));
}

// ── Assign-default (:=) edge cases ──────────────────────────────────

#[test]
fn assign_default_to_array_element_with_negative_index() {
    let mut sh = shell();
    let r = sh
        .exec(r#"a=(x y); : "${a[-1]:=zz}"; echo "${a[@]}""#)
        .unwrap();
    assert_eq!(
        (r.stdout.as_str(), r.stderr.as_str(), r.exit_code),
        ("x y\n", "", 0)
    );
}

#[test]
fn assign_default_to_array_element_with_out_of_range_negative_index_errors() {
    let (out, err, code) = run(r#"a=(x); : "${a[-9]:=z}""#);
    assert_eq!(out, "");
    assert_eq!(err, "rust-bash: line 1: a: bad array subscript\n");
    assert_eq!(code, 1);
}

#[test]
fn assign_default_with_negative_index_to_unset_variable_errors() {
    let (out, err, code) = run(r#": "${b[-1]:=z}""#);
    assert_eq!(out, "");
    assert_eq!(err, "rust-bash: line 1: b: bad array subscript\n");
    assert_eq!(code, 1);
}

#[test]
fn assign_default_to_sparse_array_element() {
    let mut sh = shell();
    let r = sh
        .exec(r#"a=(); : "${a[2]:=y}"; echo "${a[@]}|${!a[@]}""#)
        .unwrap();
    assert_eq!(
        (r.stdout.as_str(), r.stderr.as_str(), r.exit_code),
        ("y|2\n", "", 0)
    );
}

#[test]
fn assign_default_to_positional_parameter_is_ignored() {
    // DIVERGENCE?: real bash reports "cannot assign to positional parameter";
    // here the assignment is silently dropped while the value still expands.
    let (out, err, code) = run(r#"set --; : "${1:=x}"; echo "[$1]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[]\n", "", 0));
}

#[test]
fn assign_default_through_empty_indirect_target_is_ignored() {
    let (out, err, code) = run(r#"r=; : "${!r:=x}"; echo done"#);
    assert_eq!((out.as_str(), err.as_str(), code), ("done\n", "", 0));
}

// ── Error-message parameter names ───────────────────────────────────

#[test]
fn error_operator_names_positional_and_special_parameters() {
    let (out, err, code) = run(r#"set --; echo "${1:?need one}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "rust-bash: 1: need one\n", 1)
    );

    let (out, err, code) = run(r#"set --; echo "${@:?need args}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "rust-bash: @: need args\n", 1)
    );

    let (out, err, code) = run(r#"set --; echo "${*:?need args}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "rust-bash: *: need args\n", 1)
    );
}

#[test]
fn error_operator_names_array_element() {
    let (out, err, code) = run(r#"a=(); echo "${a[0]:?no elem}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "rust-bash: a[0]: no elem\n", 1)
    );
}

#[test]
fn error_operator_without_message_expands_empty_message() {
    // DIVERGENCE?: real bash prints "parameter null or not set" for `${x:?}`;
    // the parser yields an empty (rather than absent) message word here.
    let (out, err, code) = run(r#"echo "${missing:?}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "rust-bash: missing: \n", 1)
    );
}

// ── Default-value word quoting ──────────────────────────────────────

#[test]
fn double_quoted_default_treats_single_quotes_as_literal() {
    let (out, err, code) = run(r#"v=V; echo "${u:-'x$v'}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("'xV'\n", "", 0));
}

#[test]
fn double_quoted_default_escaped_closing_brace() {
    let (out, err, code) = run(r#"echo "${u:-a\}b}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("a}b\n", "", 0));
}

#[test]
fn double_quoted_default_expands_parameter_pieces() {
    let (out, err, code) = run(r#"v=V; echo "${u:-$v}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("V\n", "", 0));
}

#[test]
fn unquoted_default_splits_like_bare_words() {
    let (out, err, code) = run(r##"set -- ${u:-a b}; echo "$#|$1|$2"; v=q; echo ${u:-$v}"##);
    assert_eq!((out.as_str(), err.as_str(), code), ("2|a|b\nq\n", "", 0));
}

#[test]
fn double_quoted_error_message_keeps_single_quotes_literal() {
    let (out, err, code) = run(r#"echo "${u:?'boom'}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("", "rust-bash: u: 'boom'\n", 1)
    );
}

#[test]
fn double_quoted_replacement_keeps_single_quoted_text_literal() {
    let (out, err, code) = run(r#"v=xax; r=repl; echo "${v/a/"pre'${r}'post"}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("xpre'${r}'postx\n", "", 0)
    );
}

// ── Length/slice syntax validation ──────────────────────────────────

#[test]
fn length_of_multi_digit_positional_with_suffix_is_bad_substitution() {
    let (out, err, code) = run(r#"echo "${#10x}""#);
    assert_eq!(out, "");
    assert_eq!(err, "rust-bash: line 1: ${#10x}: bad substitution\n");
    assert_eq!(code, 1);
}

// ── Error propagation through default operators ─────────────────────

#[test]
fn empty_array_subscript_errors_propagate_through_test_operators() {
    // `${a[]...}` fails while resolving the parameter value, and the error
    // propagates out of each `:-` / `:+` / `:=` / `:?` arm.
    for (op, script) in [
        (":-", r#"a=(x); echo "${a[]:-d}""#),
        (":+", r#"a=(x); echo "${a[]:+d}""#),
        (":=", r#"a=(x); : "${a[]:=d}""#),
        (":?", r#"a=(x); echo "${a[]:?m}""#),
    ] {
        let (out, err, code) = run(script);
        assert_eq!(
            (out.as_str(), err.as_str(), code),
            ("", "rust-bash: line 1: a: bad array subscript\n", 1),
            "operator {op}"
        );
    }
}

// ── Empty default/assign words ──────────────────────────────────────

#[test]
fn empty_default_and_assign_words() {
    let mut sh = shell();
    let r = sh
        .exec(r#"echo "[${u-}]|[${u:-}]"; : "${u:=}"; declare -p u"#)
        .unwrap();
    assert_eq!(r.stdout, "[]|[]\ndeclare -- u=\"\"\n");
    assert_eq!(r.exit_code, 0);
}

// ── $0 / positional zero ────────────────────────────────────────────

#[test]
fn braced_zero_positional_is_the_shell_name() {
    let (out, err, code) = run(r#"echo "${0}|${0:-x}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("rust-bash|rust-bash\n", "", 0)
    );
}

// ── is_unset checks across parameter shapes ─────────────────────────

#[test]
fn default_operator_unset_checks_for_indexed_and_special_names() {
    let (out, err, code) = run(
        r#"echo "${FUNCNAME[0]:-d}|${LINENO[0]:-d}|${novar[3]:-d}|${FUNCNAME[@]:-d}|${LINENO[@]:-d}""#,
    );
    assert_eq!((out.as_str(), err.as_str(), code), ("d|d|d|d|d\n", "", 0));
}

#[test]
fn default_operator_unset_checks_for_negative_and_scalar_subscripts() {
    let mut sh = shell();
    let r = sh
        .exec(r#"a=(x y); s=v; echo "${a[-1]:-d}|${a[-9]:-d}|${s[1]:-d}|${s[0]:-d}""#)
        .unwrap();
    assert_eq!(r.stdout, "y|d|d|v\n");
    // The out-of-range negative subscript warns but does not fail.
    assert_eq!(r.stderr, "rust-bash: line 1: a: bad array subscript\n");
    assert_eq!(r.exit_code, 0);
}

// ── $* / ${arr[*]} alternative operator ─────────────────────────────

#[test]
fn alternative_operator_on_star_with_unset_ifs() {
    let (out, err, code) =
        run(r#"unset IFS; set -- a b; echo "${*:+x}"; a=(p q); echo "${a[*]:+y}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("x\ny\n", "", 0));
}

// ── Single-quoted content that fails inner parse ────────────────────

#[test]
fn double_quoted_default_with_unreparseable_inner_text() {
    // The inner single-quoted text is a lone `"`, which fails re-parsing as
    // a word, so it is kept literally between the literal quote characters.
    let (out, err, code) = run(r#"echo "${u:-'"'}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("'\"'\n", "", 0));
}

// ── Call-stack pseudo-arrays in script-file execution ───────────────

#[test]
fn script_file_functions_have_a_main_call_stack_frame() {
    use std::collections::HashMap;
    // Running a script file via `sh` synthesizes a "main" frame at the
    // bottom of the call-stack pseudo-arrays.
    let files: HashMap<String, Vec<u8>> = [(
        "/s.sh".to_string(),
        b"f() { echo \"${FUNCNAME[@]}|${BASH_SOURCE[@]}|${BASH_LINENO[@]}|${FUNCNAME[-1]}|${FUNCNAME[1]}\"; }; f".to_vec(),
    )]
    .into_iter()
    .collect();
    let mut sh = RustBashBuilder::new().files(files).build().unwrap();
    let r = sh.exec("sh /s.sh").unwrap();
    assert_eq!(r.stdout, "f main|/s.sh /s.sh|1 0|main|main\n");
    assert_eq!(r.exit_code, 0);
}

// ── Indirect transform over positional parameters ───────────────────

#[test]
fn quote_transform_through_indirect_at() {
    // `${!@...}` treats the positional values as indirect target names: the
    // @Q transform checks whether the *target* scalar is defined, and since
    // no variable named `x` exists, the expansion is empty.
    let (out, err, code) = run(r#"set -- x; echo "${!@@Q}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("\n", "", 0));
}

// ── Immutable path via pattern context: array slices ────────────────

#[test]
fn replacement_array_slice_negative_offset_and_no_length() {
    let (out, err, code) =
        run(r#"a=(p q r); v=axb; echo "${v/x/"${a[@]: -2}"}"; echo "${v/x/"${a[@]:1}"}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("aq rb\naq rb\n", "", 0)
    );
}

#[test]
fn vectorized_strip_keeps_non_matching_elements() {
    let (out, err, code) = run(r#"a=(axb c); echo "${a[@]%%x*}|${a[@]##*x}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("a c|b c\n", "", 0));
}

#[test]
fn pattern_scalar_subscript_beyond_zero_is_empty() {
    let (out, err, code) = run(r#"s=hi; v=ab; echo "[${v%${s[2]}}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[ab]\n", "", 0));
}

#[test]
fn pattern_indirect_expansion_of_nameref_returns_target_name() {
    let (out, err, code) = run(r#"declare -n nr=t; t=b; v=abt; echo "${v%${!nr}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

// ── Indirect expansion: special targets through test operators ──────

#[test]
fn indirect_default_with_star_and_hash_targets() {
    let (out, err, code) = run(r#"r='*'; set -- a b; echo "${!r:-d}"; r='#'; echo "${!r:-d}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("a b\n2\n", "", 0));
}

#[test]
fn indirect_default_with_last_background_target() {
    // $! is empty (no background jobs): `:-` uses the default, `-` does not.
    let (out, err, code) = run(r#"r='!'; echo "${!r:-d}|${!r-d}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("d|\n", "", 0));
}

#[test]
fn pattern_indirect_default_with_at_target() {
    let (out, err, code) = run(r#"set -- c; r=@; v=abc; echo "${v%${!r:-zz}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

// ── Assign-default (:=) negative-index resolution ───────────────────

#[test]
fn assign_default_with_negative_index_on_scalar_errors() {
    // A scalar has implicit max index 0, so -2 resolves out of range.
    let (out, err, code) = run(r#"s=v; : "${s[-2]:=w}""#);
    assert_eq!(out, "");
    assert_eq!(err, "rust-bash: line 1: s: bad array subscript\n");
    assert_eq!(code, 1);
}

#[test]
fn assign_default_with_negative_index_into_sparse_array() {
    let (out, err, code) = run(r#"a=(); a[5]=x; : "${a[-2]:=z}"; echo "${a[@]}|${!a[@]}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("z x|4 5\n", "", 0));
}

#[test]
fn assign_default_to_readonly_array_element_errors() {
    let (out, err, code) = run(r#"a=(); readonly a; : "${a[0]:=x}""#);
    assert_eq!(out, "");
    assert_eq!(err, "ERR:execution error: a: readonly variable");
    assert_eq!(code, -1);
}

// ── is_unset via the `-` (Unset) test form ──────────────────────────

#[test]
fn unset_test_form_checks_special_and_indexed_names() {
    // Unlike `:-`, the `-` form cannot short-circuit on an empty value, so
    // it always consults is_unset.
    let (out, err, code) = run(
        r#"echo "${FUNCNAME[0]-d}|${LINENO[0]-d}|${novar[3]-d}|${FUNCNAME[@]-d}|${LINENO[@]-d}""#,
    );
    assert_eq!((out.as_str(), err.as_str(), code), ("d||d|d|d\n", "", 0));
}

#[test]
fn unset_test_form_checks_negative_and_scalar_subscripts() {
    let mut sh = shell();
    let r = sh
        .exec(r#"a=(x y); s=v; echo "${a[-1]-d}|${a[-9]-d}|${s[1]-d}""#)
        .unwrap();
    assert_eq!(r.stdout, "y|d|d\n");
    assert_eq!(r.stderr, "rust-bash: line 1: a: bad array subscript\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn unset_test_form_on_special_parameters_uses_values() {
    let (out, err, code) = run(r#"echo "${?-x}"; r=0; echo "${!r-x}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("0\nrust-bash\n", "", 0)
    );
}

// ── Transforms through unusual indirect targets ─────────────────────

#[test]
fn assignment_transform_through_positional_zero_indirect_target() {
    let (out, err, code) = run(r#"r=0; echo "[${!r@A}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[]\n", "", 0));
}

#[test]
fn quote_transform_through_empty_or_indexed_indirect_at_target() {
    let (out, err, code) = run(
        r#"set -- ""; echo "[${!@@Q}]"; set -- 'A[0]'; declare -A A=([k]=v); echo "[${!@@Q}]""#,
    );
    assert_eq!((out.as_str(), err.as_str(), code), ("[]\n[]\n", "", 0));
}

// ── Immutable path via pattern context: default-word pieces ─────────

#[test]
fn pattern_double_quoted_default_piece_variants() {
    // Single quotes are literal in DQ context (content still expands), and a
    // parameter piece expands normally.
    let (out, err, code) =
        run(r#"b=B; v=abxb; echo "${v%${u:-'x$b'}}"; q=c; v=abc; echo "${v%${u:-$q}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("abxb\nab\n", "", 0));
}

#[test]
fn pattern_unquoted_default_parameter_piece() {
    let (out, err, code) = run(r#"q=b; v=abxb; echo ${v%${u:-$q}}"#);
    assert_eq!((out.as_str(), err.as_str(), code), ("abx\n", "", 0));
}

#[test]
fn pattern_within_double_quotes_default_piece_variants() {
    // Quoting the pattern itself routes the default word through the
    // double-quoted piece rules: single quotes are literal (content still
    // expands), parameter pieces expand, and `\}` yields a literal brace.
    let (out, err, code) = run(
        r#"b=B; v=abxb; echo "${v%"${u:-'x$b'}"}"; q=c; v=abc; echo "${v%"${u:-$q}"}"; v=ab; echo "${v%"${u:-a\}b}"}""#,
    );
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("abxb\nab\nab\n", "", 0)
    );
}

#[test]
fn pattern_within_double_quotes_default_with_unreparseable_inner_text() {
    // Immutable analogue of the mutable fallback: the inner single-quoted
    // text (a lone `"`) fails re-parsing and is kept literally, so the
    // pattern becomes `'"'` and does not match.
    let (out, err, code) = run(r#"v=ab; echo "${v%"${u:-'"'}"}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

#[test]
fn pattern_default_with_unbalanced_inner_text() {
    // The inner single-quoted text `${` re-parses to a literal Text piece
    // (brush degrades unparseable `$` constructs to text), so the pattern
    // becomes `'${'` and does not match.
    let (out, err, code) = run(r#"v=ab; echo "${v%${u:-'${'}}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

#[test]
fn gettext_double_quoted_string_in_pattern_context() {
    let (out, err, code) = run(r#"v=abc; echo "${v%$"c"}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

// ── Pattern replacement strings: remaining corners ──────────────────

#[test]
fn replacement_parameter_reference_stops_at_punctuation() {
    let (out, err, code) = run(r#"r=R; v=axb; echo "${v/x/$r!}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("aR!b\n", "", 0));
}

#[test]
fn replacement_star_joins_with_space_when_ifs_unset() {
    let (out, err, code) = run(r#"unset IFS; set -- p q; v=axb; echo "${v/x/$*}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ap qb\n", "", 0));
}

#[test]
fn replacement_ansic_quotes_expand_all_escape_kinds() {
    let (out, err, code) = run(
        r#"v=axb; echo "${v/x/$'\n\r\a\b\e\E\f\v\z'}"; echo "${v/x/$'\\'}"; echo "${v/x/$'\''}"; echo "${v/x/$'q'}""#,
    );
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        (
            "a\n\r\u{7}\u{8}\u{1b}\u{1b}\u{c}\u{b}\\zb\na\\b\na'b\naqb\n",
            "",
            0
        )
    );
}

// ── Case-modification patterns with extglob disabled ────────────────

#[test]
fn case_modification_pattern_uses_plain_glob_without_extglob() {
    let (out, err, code) = run(r#"shopt -u extglob; x=abc; echo "${x^^[ab]}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ABc\n", "", 0));
}

// ── Array slice: negative offset with length ────────────────────────

#[test]
fn array_slice_negative_offset_with_length_beyond_bounds_is_empty() {
    let (out, err, code) = run(r#"a=(x y); echo "[${a[@]: -5:2}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[]\n", "", 0));
}

// ── Call-stack pseudo-arrays: star join with unset IFS ──────────────

#[test]
fn funcname_star_joins_with_space_when_ifs_unset() {
    let (out, err, code) = run(r#"f() { unset IFS; g; }; g() { echo "${FUNCNAME[*]}"; }; f"#);
    assert_eq!((out.as_str(), err.as_str(), code), ("g f\n", "", 0));
}

// ── is_unset via indirect validation ────────────────────────────────

#[test]
fn indirect_expansion_of_call_stack_arrays_checks_unset() {
    // FUNCNAME counts as unset outside a function, so `${!FUNCNAME[@]-d}`
    // fails indirect validation; LINENO is dynamically always set.
    let (out, err, code) = run(r#"echo "${!FUNCNAME[@]-d}""#);
    assert_eq!(out, "");
    assert_eq!(
        err,
        "rust-bash: line 1: FUNCNAME: invalid indirect expansion\n"
    );
    assert_eq!(code, 1);

    let (out, err, code) = run(r#"echo "${!LINENO[@]-d}"; arr=(x); echo "${!arr[@]-d}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("d\nd\n", "", 0));
}

// ── Scalar/empty slice key-value pairs ──────────────────────────────

#[test]
fn array_slice_of_empty_scalar_is_empty() {
    let (out, err, code) = run(r#"s=; echo "[${s[@]:0:1}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[]\n", "", 0));
}
