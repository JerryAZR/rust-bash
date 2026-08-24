//! Coverage-triage tests for `src/interpreter/builtins.rs`.
//!
//! Every test here exists to cover a previously-uncovered region of the
//! builtins implementation: untested flag combinations and error paths.
//! Where behavior diverges from real bash, the actual behavior is pinned
//! with a comment (runtime behavior is intentionally not changed).

use rust_bash::{ExecResult, RustBash, RustBashBuilder};

fn shell() -> RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> ExecResult {
    shell().exec(script).unwrap()
}

// ── check_help / stub --help ────────────────────────────────────────

#[test]
fn stub_help_for_registered_command() {
    // /bin/grep is a stub file; running it with --help routes through
    // execute_path_command -> check_help -> registered-command meta arm.
    let r = run("/bin/grep --help");
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.contains("Usage:"), "stdout: {:?}", r.stdout);
    assert!(r.stdout.contains("grep"), "stdout: {:?}", r.stdout);
}

// ── cd ──────────────────────────────────────────────────────────────

#[test]
fn cd_home_not_set() {
    let r = run("unset HOME; cd");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "cd: HOME not set\n");
}

#[test]
fn cd_cdpath_match_prints_directory() {
    let r = run("mkdir -p /lib/foo; CDPATH=/lib; cd foo; pwd");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, "/lib/foo\n/lib/foo\n");
}

#[test]
fn cd_cdpath_candidate_that_is_not_a_directory_is_skipped() {
    let r = run("mkdir /lib; touch /lib/foo; CDPATH=/lib; cd foo");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "cd: foo: No such file or directory\n");
}

#[test]
fn cd_relative_path_with_dot_component() {
    let r = run("mkdir -p /x/a/b; cd /x; cd a/./b; pwd");
    assert_eq!(r.stdout, "/x/a/b\n");
}

#[test]
fn cd_relative_path_with_dotdot_component() {
    let r = run("mkdir -p /x/a /x/c; cd /x; cd a/../c; pwd");
    assert_eq!(r.stdout, "/x/c\n");
}

#[test]
fn cd_multi_component_relative_path() {
    let r = run("mkdir -p /m/n/o; cd /m; cd n/o; pwd");
    assert_eq!(r.stdout, "/m/n/o\n");
}

#[test]
fn cd_to_regular_file_not_a_directory() {
    let r = run("touch /f; cd /f");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "cd: /f: Not a directory\n");
}

// ── export ──────────────────────────────────────────────────────────

#[test]
fn export_append_to_readonly_errors() {
    let r = run("readonly ro=1; export ro+=2");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "rust-bash: ro: readonly variable\n");
}

#[test]
fn export_append_propagates_array_limit() {
    let mut sh = RustBashBuilder::new()
        .max_array_elements(2)
        .build()
        .unwrap();
    sh.exec("arr=(a b)").unwrap();
    // Non-Execution errors (LimitExceeded) propagate out of exec.
    let err = sh.exec("export arr+=(c)").unwrap_err();
    assert!(
        matches!(
            err,
            rust_bash::RustBashError::LimitExceeded {
                limit_name: "max_array_elements",
                ..
            }
        ),
        "got: {err:?}"
    );
}

#[test]
fn export_subscript_assignment_skips_attribute_update() {
    // `export 'arr[0]=x'` assigns the element; the attribute update looks
    // up the literal name `arr[0]`, which never exists.
    let r = run("export 'earr[0]=x'; echo \"${earr[0]}\"");
    assert_eq!(r.stdout, "x\n");
}

#[test]
fn export_unexport_with_assignment() {
    // `export -n y=5` assigns but does not export.
    let mut sh = shell();
    let r = sh.exec("export -n y=5; echo $y").unwrap();
    assert_eq!(r.stdout, "5\n");
    let r = sh.exec("sh -c 'echo [${y-unset}]'").unwrap();
    assert_eq!(r.stdout, "[unset]\n");
}

#[test]
fn export_unexport_existing_variable() {
    let r = run("export z=1; export -n z; sh -c 'echo [${z-unset}]'");
    assert_eq!(r.stdout, "[unset]\n");
}

// ── unset ───────────────────────────────────────────────────────────

#[test]
fn unset_unknown_flag_is_silently_consumed() {
    // Divergence: bash reports `unset: -q: invalid option` (exit 2);
    // rust-bash silently consumes unknown dash-flags.
    let r = run("foo=bar; unset -q foo; echo \"rc=$? [${foo-unset}]\"");
    assert_eq!(r.stdout, "rc=0 [unset]\n");
}

#[test]
fn unset_invalid_identifier_with_subscript() {
    let r = run("unset '1bad[0]'");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "unset: `1bad[0]': not a valid identifier\n");
}

#[test]
fn unset_readonly_array_element() {
    let r = run("declare -ar arr=(a b); unset 'arr[0]'");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "unset: arr: cannot unset: readonly variable\n");
}

#[test]
fn unset_negative_index_on_scalar() {
    // A negative subscript on a scalar has no array to resolve against.
    let r = run("s=foo; unset 's[-1]'; echo \"rc=$? s=$s\"");
    assert_eq!(r.stdout, "rc=1 s=foo\n");
    assert_eq!(
        r.stderr,
        "rust-bash: line 1: unset: [-1]: bad array subscript\n"
    );
}

#[test]
fn unset_negative_index_on_empty_array() {
    let r = run("declare -a empty=(); unset 'empty[-1]'; echo \"rc=$?\"");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn unset_scalar_element_zero_clears_value() {
    let r = run("s=foo; unset 's[0]'; echo \"[$s]\"");
    assert_eq!(r.stdout, "[]\n");
}

#[test]
fn unset_nameref_to_readonly_target() {
    let r = run("readonly tgt=1; declare -n ref=tgt; unset ref");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "unset: tgt: cannot unset: readonly variable\n");
}

#[test]
fn unset_nameref_pointing_at_itself() {
    let r = run("declare -n selfref=selfref; unset selfref; echo rc=$?");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn unset_enclosing_function_local_from_inner_call() {
    // `g` unsets the local that `f` declared (dynamic scoping). The saved
    // pre-`local` value was "did not exist", so the variable is removed.
    let r = run("g() { unset lv; }; f() { local lv; lv=in_f; g; echo \"[${lv-unset}]\"; }; f");
    assert_eq!(r.stdout, "[unset]\n");
}

#[test]
fn unset_local_with_temp_binding_without_prior_value() {
    let r = run(
        "h() { local tv; unset tv; echo \"inner:[${tv-unset}]\"; }; tv=tmp h; echo \"outer:[${tv-unset}]\"",
    );
    assert_eq!(r.stdout, "inner:[unset]\nouter:[unset]\n");
}

#[test]
fn unset_posix_temp_binding_at_top_level() {
    let r = run("set -o posix; pv=orig; pv=tmp unset pv; echo \"[${pv-unset}]\"");
    assert_eq!(r.stdout, "[unset]\n");
}

#[test]
fn unset_declared_only_local_indexed_array_shadow() {
    let r = run("f() { local -a la; la=(x); unset la; echo \"[${la-unset}]\"; }; f; echo done");
    assert_eq!(r.stdout, "[unset]\ndone\n");
}

#[test]
fn unset_declared_only_local_assoc_array_shadow() {
    let r = run("f() { local -A lA; lA[k]=v; unset lA; echo \"[${lA-unset}]\"; }; f; echo done");
    assert_eq!(r.stdout, "[unset]\ndone\n");
}

// ── set (listing / quoting) ─────────────────────────────────────────

#[test]
fn set_listing_ansi_c_quotes_control_chars() {
    let r = run("x=$'\\a\\b\\v\\f\\e\\1'; set | grep '^x='");
    assert_eq!(r.stdout, "x=$'\\a\\b\\v\\f\\E\\001'\n");
}

#[test]
fn set_listing_octal_escapes_non_utf8_marker_bytes() {
    // printf '\377' produces raw byte 0xFF, kept as a private-use marker
    // char inside String values; `set` renders it back as an octal escape.
    let r = run("x=$(printf '\\377'); set | grep '^x='");
    assert_eq!(r.stdout, "x=$'\\377'\n");
}

#[test]
fn set_listing_double_quotes_array_value_special_chars() {
    // Array values are rendered double-quoted by `set` unless they need
    // ansi-c quoting; scalar values are single-quoted instead.
    let r = run("declare -a dq; dq[0]='a$b'; set | grep '^dq='");
    assert_eq!(r.stdout, "dq=([0]=\"a\\$b\")\n");
}

#[test]
fn set_listing_ansi_c_quotes_assoc_key() {
    let r = run("declare -A m; m[$'\\x01']=v; set | grep '^m='");
    assert_eq!(r.stdout, "m=([$'\\001']=\"v\" )\n");
}

#[test]
fn set_listing_empty_assoc_array() {
    let r = run("declare -A em=(); set | grep '^em='");
    assert_eq!(r.stdout, "em=()\n");
}

#[test]
fn set_plus_o_without_arg_lists_reparseable_options() {
    let r = run("set +o");
    assert_eq!(r.exit_code, 0);
    assert!(
        r.stdout.contains("set +o errexit\n"),
        "stdout: {:?}",
        r.stdout
    );
    assert!(
        r.stdout.contains("set +o pipefail\n"),
        "stdout: {:?}",
        r.stdout
    );
}

#[test]
fn set_unknown_option_char_is_silently_ignored() {
    // Divergence: bash reports `set: -Z: invalid option` (exit 2);
    // rust-bash silently ignores unknown option chars.
    let r = run("set -Z; echo rc=$?");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn set_errexit_under_bang_suppression_inside_function() {
    // Enabling errexit while `!`-suppression is active inside a function
    // decrements the suppression counter (errexit_bang_suppressed).
    let r = run("f() { ! set -o errexit; echo after; }; f; echo rc=$?");
    assert_eq!(r.stdout, "after\nrc=0\n");
}

// ── wait ────────────────────────────────────────────────────────────

#[test]
fn wait_with_non_numeric_pid() {
    let r = run("wait abc");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "wait: abc: not a pid or valid job spec\n");
}

#[test]
fn wait_with_unknown_numeric_pid() {
    let r = run("wait 4242; echo rc=$?");
    assert_eq!(r.stdout, "rc=127\n");
}

// ── readonly ────────────────────────────────────────────────────────

#[test]
fn readonly_bare_dash_is_invalid_identifier() {
    // Divergence: bash treats `readonly -` as `readonly` with no names
    // (prints all readonly vars); rust-bash treats `-` as a name and
    // rejects it.
    let r = run("readonly -");
    assert_eq!(r.exit_code, 1);
    assert_eq!(
        r.stderr,
        "rust-bash: readonly: `-': not a valid identifier\n"
    );
}

#[test]
fn readonly_unknown_flag_char_is_ignored() {
    // Divergence: bash reports `readonly: -z: invalid option` (exit 2);
    // rust-bash silently ignores unknown flag chars.
    let r = run("readonly -z rv=1; echo $rv");
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn readonly_append_to_readonly_errors() {
    let r = run("readonly ra=1; readonly ra+=2");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "rust-bash: ra: readonly variable\n");
}

#[test]
fn readonly_append_propagates_array_limit() {
    let mut sh = RustBashBuilder::new()
        .max_array_elements(2)
        .build()
        .unwrap();
    sh.exec("declare -a rarr=(a b)").unwrap();
    let err = sh.exec("readonly rarr+=(c)").unwrap_err();
    assert!(
        matches!(
            err,
            rust_bash::RustBashError::LimitExceeded {
                limit_name: "max_array_elements",
                ..
            }
        ),
        "got: {err:?}"
    );
}

#[test]
fn readonly_assign_to_existing_readonly_errors() {
    let r = run("readonly rb=1; readonly rb=2");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "execution error: rb: readonly variable\n");
}

// ── declare ─────────────────────────────────────────────────────────

#[test]
fn declare_bare_dash_is_invalid_identifier() {
    let r = run("declare - dx");
    assert_eq!(r.exit_code, 1);
    assert!(
        r.stderr.contains("not a valid identifier"),
        "stderr: {:?}",
        r.stderr
    );
}

#[test]
fn declare_plus_invalid_option() {
    let r = run("declare +z x");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "rust-bash: declare: +z: invalid option\n");
}

#[test]
fn declare_append_inside_function_records_local_scope() {
    let r = run("f() { declare lv=1; declare lv+=2; echo $lv; }; f");
    assert_eq!(r.stdout, "12\n");
}

#[test]
fn declare_append_scalar_to_assoc_errors() {
    let r = run("declare -A am=([k]=v); declare am+=scalar");
    assert_eq!(r.exit_code, 1);
    assert_eq!(
        r.stderr,
        "rust-bash: am: cannot append scalar to associative array\n"
    );
}

#[test]
fn declare_append_propagates_array_limit() {
    let mut sh = RustBashBuilder::new()
        .max_array_elements(2)
        .build()
        .unwrap();
    sh.exec("declare -a la=(a b)").unwrap();
    let err = sh.exec("declare la+=(c)").unwrap_err();
    assert!(
        matches!(
            err,
            rust_bash::RustBashError::LimitExceeded {
                limit_name: "max_array_elements",
                ..
            }
        ),
        "got: {err:?}"
    );
}

#[test]
fn declare_assign_propagates_array_limit() {
    let mut sh = RustBashBuilder::new()
        .max_array_elements(2)
        .build()
        .unwrap();
    let err = sh.exec("declare -a lb=(a b c)").unwrap_err();
    assert!(
        matches!(
            err,
            rust_bash::RustBashError::LimitExceeded {
                limit_name: "max_array_elements",
                ..
            }
        ),
        "got: {err:?}"
    );
}

#[test]
fn declare_f_lists_function_definitions() {
    let r = run("fntest() { :; }; declare -f");
    assert_eq!(r.stdout, "fntest () { :; }\n");
}

#[test]
fn declare_p_shows_uppercase_attribute() {
    let r = run("declare -u up=abc; declare -p up");
    assert_eq!(r.stdout, "declare -u up=\"ABC\"\n");
}

#[test]
fn declare_p_assoc_reverse_print_large_key_set() {
    // Exercises the ASSOC_REVERSE_PRINT ordering (key "0" first, the rest
    // in reverse lexicographic order) across a larger key set.
    let mut script = String::from("declare -A rl=([0]=z");
    for i in 1..=30 {
        script.push_str(&format!(" [{i}]=v{i}"));
    }
    script.push_str("); declare -p rl");
    let r = run(&script);
    let mut keys: Vec<String> = (1..=30).map(|i| i.to_string()).collect();
    keys.sort_by(|a, b| b.cmp(a));
    let mut expected = String::from("declare -A rl=([0]=\"z\"");
    for k in &keys {
        expected.push_str(&format!(" [{k}]=\"v{k}\""));
    }
    expected.push_str(" )\n");
    assert_eq!(r.stdout, expected);
}

#[test]
fn declare_p_assoc_reverse_print_orders_zero_key_first() {
    // ASSOC_REVERSE_PRINT (set when the literal used unquoted keys) sorts
    // key "0" first, then the rest in reverse.
    let r = run("declare -A rz=([0]=a [5]=b [4]=c [3]=d [2]=e [1]=f); declare -p rz");
    assert_eq!(
        r.stdout,
        "declare -A rz=([0]=\"a\" [5]=\"b\" [4]=\"c\" [3]=\"d\" [2]=\"e\" [1]=\"f\" )\n"
    );
}

#[test]
fn declare_assoc_append_on_indexed_errors() {
    let r = run("declare -a ix=(1); declare -A ix+=([k]=v)");
    assert_eq!(r.exit_code, 1);
    assert_eq!(
        r.stderr,
        "rust-bash: ix: cannot convert indexed array to associative array\n"
    );
}

#[test]
fn declare_assoc_append_creates_variable() {
    let r = run("declare -A newm+=([k]=v); echo ${newm[k]}");
    assert_eq!(r.stdout, "v\n");
}

#[test]
fn declare_assoc_append_converts_existing_scalar() {
    let r = run("cs=foo; declare -A cs+=([k]=v); echo \"${cs[0]}/${cs[k]}\"");
    assert_eq!(r.stdout, "foo/v\n");
}

#[test]
fn declare_readonly_assoc_append() {
    let r = run("declare -rA rm+=([k]=v); declare -p rm");
    assert_eq!(r.stdout, "declare -Ar rm=([k]=\"v\" )\n");
}

#[test]
fn declare_indexed_append_on_assoc_is_silently_ignored() {
    // Divergence: bash errors with `must use subscript when assigning
    // associative array`; rust-bash treats the literal as an assoc append
    // and ignores the subscript-less words.
    let r = run("declare -A aa=([k]=v); declare aa+=(x y); echo \"rc=$? n=${#aa[@]}\"");
    assert_eq!(r.stdout, "rc=0 n=1\n");
}

#[test]
fn declare_assoc_with_nonliteral_value() {
    // Divergence: bash errors with `must use subscript when assigning
    // associative array`; rust-bash creates an empty assoc array.
    let r = run("declare -A an=plain; echo \"rc=$? n=${#an[@]}\"");
    assert_eq!(r.stdout, "rc=0 n=0\n");
}

#[test]
fn declare_readonly_assoc_literal() {
    let r = run("declare -rA rl=([k]=v); declare -p rl");
    assert_eq!(r.stdout, "declare -Ar rl=([k]=\"v\" )\n");
}

#[test]
fn declare_indexed_with_empty_value() {
    let r = run("declare -a ie=; echo \"rc=$? n=${#ie[@]}\"");
    assert_eq!(r.stdout, "rc=0 n=0\n");
}

#[test]
fn declare_indexed_append_replaces_existing_scalar() {
    let r = run("sx=x; declare sx+=(a b); echo \"${sx[@]}\"");
    assert_eq!(r.stdout, "x a b\n");
}

#[test]
fn declare_indexed_literal_bare_subscript_is_ignored() {
    let r = run("declare -a bj=([5] x); echo \"${bj[0]} n=${#bj[@]}\"");
    assert_eq!(r.stdout, "x n=1\n");
}

#[test]
fn declare_indexed_literal_without_flag_replaces_existing_scalar() {
    let r = run("sc2=x; declare sc2=(a b); echo ${sc2[1]}");
    assert_eq!(r.stdout, "b\n");
}

#[test]
fn declare_indexed_replaces_existing_scalar() {
    let r = run("sc=x; declare -a sc=(a b); echo ${sc[1]}");
    assert_eq!(r.stdout, "b\n");
}

#[test]
fn declare_nameref_on_array_is_error() {
    let r = run("declare -a na; declare -n na");
    assert_eq!(r.exit_code, 1);
    assert_eq!(
        r.stderr,
        "rust-bash: declare: nameref variable cannot be an array\n"
    );
}

#[test]
fn declare_indexed_on_assoc_is_error() {
    let r = run("declare -A cn=([k]=v); declare -a cn");
    assert_eq!(r.exit_code, 1);
    assert_eq!(
        r.stderr,
        "rust-bash: declare: cn: cannot convert associative array to indexed array\n"
    );
}

#[test]
fn declare_assoc_converts_existing_scalar() {
    let r = run("cv=1; declare -A cv; declare -p cv");
    assert_eq!(r.stdout, "declare -A cv=()\n");
}

#[test]
fn declare_indexed_converts_existing_scalar() {
    let r = run("cw=1; declare -a cw; declare -p cw");
    assert_eq!(r.stdout, "declare -a cw=()\n");
}

#[test]
fn declare_nameref_rejects_unclosed_subscript_target() {
    let r = run("bn='arr[1'; declare -n bn");
    assert_eq!(r.exit_code, 1);
    assert!(
        r.stderr.contains("not a valid identifier"),
        "stderr: {:?}",
        r.stderr
    );
}

#[test]
fn declare_nameref_rejects_empty_name_before_subscript() {
    let r = run("bn2='[0]'; declare -n bn2");
    assert_eq!(r.exit_code, 1);
    assert!(
        r.stderr.contains("not a valid identifier"),
        "stderr: {:?}",
        r.stderr
    );
}

#[test]
fn declare_indexed_literal_ignores_nonnumeric_explicit_index() {
    let r = run("declare -a bi=([foo]=v x); echo \"${bi[0]}-${#bi[@]}\"");
    assert_eq!(r.stdout, "x-1\n");
}

#[test]
fn declare_assoc_literal_bare_key_gets_empty_value() {
    let r = run("declare -A bk=([k1] [k2]=v); echo \"[${bk[k1]}]${bk[k2]}\"");
    assert_eq!(r.stdout, "[]v\n");
}

#[test]
fn declare_assoc_append_bare_key_gets_empty_value() {
    let r = run("declare -A ab=([a]=1); declare ab+=([b]); echo \"${ab[a]}[${ab[b]}]\"");
    assert_eq!(r.stdout, "1[]\n");
}

#[test]
fn declare_assoc_append_malformed_key_is_ignored() {
    let r = run("declare -A mm=([a]=1); declare mm+=([bad); echo \"rc=$? n=${#mm[@]}\"");
    assert_eq!(r.stdout, "rc=0 n=1\n");
}

#[test]
fn declare_assoc_literal_malformed_key_is_ignored() {
    let r = run("declare -A mk=([k); echo \"rc=$? n=${#mk[@]}\"");
    assert_eq!(r.stdout, "rc=0 n=0\n");
}

#[test]
fn declare_indexed_literal_backslash_after_dquote_char() {
    // An element whose expanded value contains a literal `"` followed by a
    // backslash exercises the escaped-char path of the body splitter.
    let r = run("declare -a dq=('x\"y\\z'); echo \"${dq[0]}\"");
    assert_eq!(r.stdout, "x\"y\\z\n");
}

#[test]
fn declare_indexed_literal_backslash_inside_double_quotes() {
    let r = run("declare -a eb=(\"a\\\\b\"); echo ${eb[0]}");
    assert_eq!(r.stdout, "a\\b\n");
}

#[test]
fn declare_indexed_literal_escaped_quote_inside_double_quotes() {
    let r = run("declare -a eq=(\"a\\\"b\"); echo ${eq[0]}");
    assert_eq!(r.stdout, "a\"b\n");
}

// ── local (print mode & arrays) ─────────────────────────────────────

#[test]
fn local_print_mode_lists_assoc_array() {
    let r = run("f() { local -A lm=([k1]=v1); local; }; f");
    assert_eq!(r.stdout, "lm=([k1]=v1 )\n");
}

#[test]
fn local_print_mode_quotes_non_plain_assoc_key() {
    // Note: builtin_local's assoc literal parser keeps the quotes in the
    // key (it does not unquote), so the stored key is `"k x"` — pinned.
    let r = run("f() { local -A lm=([\"k x\"]=v); local; }; f");
    assert_eq!(r.stdout, "lm=(['\"k x\"']=v )\n");
}

#[test]
fn local_print_mode_empty_assoc_array() {
    let r = run("f() { local -A le; local; }; f");
    assert_eq!(r.stdout, "le=()\n");
}

// ── read ────────────────────────────────────────────────────────────

#[test]
fn read_double_dash_terminates_flags() {
    let r = run("printf 'a b\\n' | { read -- x y; echo \"$x/$y\"; }");
    assert_eq!(r.stdout, "a/b\n");
}

#[test]
fn read_array_name_attached_to_flag() {
    let r = run("printf 'a b\\n' | { read -aarr; echo \"${arr[1]}\"; }");
    assert_eq!(r.stdout, "b\n");
}

#[test]
fn read_delimiter_attached_to_flag() {
    let r = run("printf 'a,b' | { read -d, x; echo \"$x\"; }");
    assert_eq!(r.stdout, "a\n");
}

#[test]
fn read_n_without_count_defaults_to_zero() {
    let r = run("printf 'hi\\n' | { read -n; echo \"rc=$? [$REPLY]\"; }");
    assert_eq!(r.stdout, "rc=0 []\n");
}

#[test]
fn read_big_n_attached_count() {
    let r = run("printf 'hello\\n' | { read -N3 x; echo \"$x\"; }");
    assert_eq!(r.stdout, "hel\n");
}

#[test]
fn read_big_n_without_count_defaults_to_zero() {
    let r = run("printf 'hi\\n' | { read -N; echo \"rc=$? [$REPLY]\"; }");
    assert_eq!(r.stdout, "rc=0 []\n");
}

#[test]
fn read_big_n_invalid_count() {
    let r = run("printf 'hi\\n' | { read -N zz; }");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "read: zz: invalid count\n");
}

#[test]
fn read_timeout_attached_value() {
    let r = run("printf 'hi\\n' | { read -t0.5 x; echo \"$x\"; }");
    assert_eq!(r.stdout, "hi\n");
}

#[test]
fn read_timeout_without_value_returns_success_without_reading() {
    // `read -t` with no following arg parses as timeout 0 and returns
    // success immediately without consuming input.
    let r = run("printf 'hi\\n' | { read -t; echo \"rc=$? [$REPLY]\"; }");
    assert_eq!(r.stdout, "rc=0 []\n");
}

#[test]
fn read_timeout_invalid_value() {
    let r = run("printf 'hi\\n' | { read -t zz x; }");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "read: zz: invalid timeout\n");
}

#[test]
fn read_fd_attached_to_flag() {
    let r = run("read -u3 x 3<<<'hello there'; echo $x");
    assert_eq!(r.stdout, "hello there\n");
}

#[test]
fn read_unknown_flag_is_silently_ignored() {
    // Divergence: bash reports `read: -z: invalid option` (exit 2);
    // rust-bash silently ignores unknown flag chars.
    let r = run("printf 'hi\\n' | { read -z x; echo \"rc=$? $x\"; }");
    assert_eq!(r.stdout, "rc=0 hi\n");
}

#[test]
fn read_array_with_empty_ifs_is_single_field() {
    let r = run("printf 'a b\\n' | { IFS= read -a arr; echo \"${#arr[@]}:${arr[0]}\"; }");
    assert_eq!(r.stdout, "1:a b\n");
}

#[test]
fn read_u0_reads_from_stdin() {
    let r = run("printf 'hi\\n' | { read -u 0 x; echo \"$x\"; }");
    assert_eq!(r.stdout, "hi\n");
}

#[test]
fn read_u_persistent_fd_reads_file() {
    let r = run("echo line1 > /f; exec 3</f; read -u 3 x; echo \"$x\"");
    assert_eq!(r.stdout, "line1\n");
}

#[test]
fn read_u_unopened_fd_hits_eof() {
    let r = run("read -u 9 x; echo \"rc=$? [$x]\"");
    assert_eq!(r.stdout, "rc=1 []\n");
}

#[test]
fn read_u_dup_of_stdin() {
    let r = run("printf 'hi\\n' | { exec 3<&0; read -u 3 x; echo \"$x\"; }");
    assert_eq!(r.stdout, "hi\n");
}

#[test]
fn read_u_output_fd_yields_empty_input() {
    let r = run("exec 3>/tmp/out; read -u 3 x; echo \"rc=$? [$x]\"");
    assert_eq!(r.stdout, "rc=1 []\n");
}

#[test]
fn read_input_ending_in_backslash() {
    // A trailing backslash with no following char is consumed; read hits
    // EOF (rc=1) and assigns what it accumulated.
    let r = run("printf 'ab\\\\' | { read x; echo \"rc=$? [$x]\"; }");
    assert_eq!(r.stdout, "rc=1 [ab]\n");
}

#[test]
fn read_last_var_with_single_ifs_char_remainder_gets_empty() {
    let r = run("printf 'a:\\n' | { IFS=: read a b; echo \"[$a][$b]\"; }");
    assert_eq!(r.stdout, "[a][]\n");
}

// ── eval ────────────────────────────────────────────────────────────

#[test]
fn eval_double_dash_with_no_args_succeeds() {
    let r = run("eval --; echo rc=$?");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn eval_with_no_args_succeeds() {
    let r = run("eval; echo rc=$?");
    assert_eq!(r.stdout, "rc=0\n");
}

// ── trap ────────────────────────────────────────────────────────────

#[test]
fn trap_with_command_but_no_signal_is_usage_error() {
    let r = run("trap 'echo hi'");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "trap: usage: trap [-lp] [[arg] signal_spec ...]\n"
    );
}

// ── shopt ───────────────────────────────────────────────────────────

#[test]
fn shopt_query_strict_all() {
    let r = run("shopt -q strict:all; echo rc=$?");
    assert_eq!(r.stdout, "rc=1\n");
}

#[test]
fn shopt_long_unset_flag() {
    let r = run("shopt -s nullglob; shopt --unset nullglob; shopt -q nullglob; echo rc=$?");
    assert_eq!(r.stdout, "rc=1\n");
}

#[test]
fn shopt_long_query_flag() {
    let r = run("shopt --query nullglob; echo rc=$?");
    assert_eq!(r.stdout, "rc=1\n");
}

#[test]
fn shopt_long_print_flag() {
    let r = run("shopt --print nullglob");
    assert_eq!(r.stdout, "shopt -u nullglob\n");
}

#[test]
fn shopt_double_dash_terminates_flags() {
    let r = run("shopt -- nullglob");
    assert_eq!(r.stdout, "nullglob                off\n");
}

#[test]
fn shopt_invalid_option() {
    let r = run("shopt -Z");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "shopt: -Z: invalid option\n");
}

#[test]
fn shopt_s_without_names_lists_enabled() {
    let r = run("shopt -s nullglob; shopt -s | grep nullglob");
    assert_eq!(r.stdout, "nullglob            on\n");
}

#[test]
fn shopt_u_without_names_lists_disabled() {
    let r = run("shopt -u | head -2");
    assert_eq!(
        r.stdout,
        "assoc_expand_once   off\nautocd              off\n"
    );
}

#[test]
fn shopt_u_invalid_option_name() {
    let r = run("shopt -u nosuchopt");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "shopt: nosuchopt: invalid shell option name\n");
}

#[test]
fn shopt_q_with_all_names_on_falls_through_to_success() {
    let r = run("shopt -s nullglob; shopt -q nullglob; echo rc=$?");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn shopt_q_without_names_succeeds() {
    let r = run("shopt -q; echo rc=$?");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn shopt_o_s_without_names_lists_enabled_set_options() {
    let r = run("shopt -o -s | head -2");
    assert_eq!(r.stdout, "braceexpand         on\nhashall             on\n");
    let r = run("set -e; shopt -o -s | grep errexit");
    assert_eq!(r.stdout, "errexit             on\n");
}

#[test]
fn shopt_o_s_enables_set_option() {
    let r = run("shopt -o -s errexit; shopt -o -q errexit; echo rc=$?");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn shopt_o_s_invalid_option_name() {
    let r = run("shopt -o -s badname");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "shopt: badname: invalid shell option name\n");
}

#[test]
fn shopt_o_u_without_names_lists_disabled_set_options() {
    let r = run("shopt -o -u | head -2");
    assert_eq!(
        r.stdout,
        "allexport           off\nemacs               off\n"
    );
}

#[test]
fn shopt_o_u_disables_set_option() {
    let r = run("set -e; shopt -o -u errexit; shopt -o -q errexit; echo rc=$?");
    assert_eq!(r.stdout, "rc=1\n");
}

#[test]
fn shopt_o_u_invalid_option_name() {
    let r = run("shopt -o -u badname");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "shopt: badname: invalid shell option name\n");
}

#[test]
fn shopt_o_q_reports_option_state() {
    let r = run("shopt -o -q errexit; echo rc=$?");
    assert_eq!(r.stdout, "rc=1\n");
    let r = run("set -e; shopt -o -q errexit; echo rc=$?");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn shopt_o_q_invalid_option_name() {
    let r = run("shopt -o -q badname");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "shopt: badname: invalid shell option name\n");
}

#[test]
fn shopt_o_q_without_names_succeeds() {
    let r = run("shopt -o -q; echo rc=$?");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn shopt_o_without_args_lists_tabular() {
    let r = run("shopt -o | head -2");
    assert_eq!(
        r.stdout,
        "allexport           off\nbraceexpand         on\n"
    );
}

// ── source ──────────────────────────────────────────────────────────

#[test]
fn source_with_empty_filename() {
    let r = run("source ''");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "source: : No such file or directory\n");
}

// ── local ───────────────────────────────────────────────────────────

#[test]
fn local_bare_dash_is_invalid_identifier() {
    let r = run("f() { local - ; }; f");
    assert_eq!(r.exit_code, 1);
    assert!(
        r.stderr.contains("not a valid identifier"),
        "stderr: {:?}",
        r.stderr
    );
}

#[test]
fn local_assoc_literal_with_value() {
    let r = run("f() { local -A la=([k]=v); echo ${la[k]}; }; f");
    assert_eq!(r.stdout, "v\n");
}

#[test]
fn local_with_readonly_export_integer_flags() {
    let r = run("f() { local -rx rxi=5; local -i ii=3; echo $rxi$ii; }; f");
    assert_eq!(r.stdout, "53\n");
}

#[test]
fn local_outside_function_with_no_args_prints_nothing() {
    // Divergence: bash errors with `local: can only be used in a function`;
    // rust-bash exits 0 with no output.
    let r = run("local; echo rc=$?");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn local_array_append_onto_empty_scalar() {
    let r = run("ge=''; f() { local ge+=(a b); echo \"${ge[@]}\"; }; f");
    assert_eq!(r.stdout, "a b\n");
}

#[test]
fn local_array_append_onto_nonempty_scalar() {
    let r = run("gs=z; f() { local gs+=(a b); echo \"${gs[@]}\"; }; f");
    assert_eq!(r.stdout, "a b\n");
}

#[test]
fn local_array_append_onto_assoc_variable() {
    // Divergence: bash reports `local: ga: cannot convert associative to
    // indexed array` on stderr with exit 1; rust-bash propagates an
    // Execution error out of exec.
    let err = shell()
        .exec("declare -A ga=([k]=v); f() { local ga+=(a b); }; f")
        .unwrap_err();
    assert!(
        matches!(err, rust_bash::RustBashError::Execution(ref msg) if msg == "ga: cannot use numeric index on associative array"),
        "got: {err:?}"
    );
}

#[test]
fn local_assoc_with_nonliteral_value_assigns_scalar() {
    // Divergence: bash errors with `must use subscript when assigning
    // associative array`; rust-bash assigns a plain scalar.
    let r = run("f() { local -A an=plain; echo \"rc=$? $an\"; }; f");
    assert_eq!(r.stdout, "rc=0 plain\n");
}

#[test]
fn local_indexed_with_nonliteral_value_assigns_scalar() {
    let r = run("f() { local -a iv=plain; echo \"rc=$? ${iv[0]}\"; }; f");
    assert_eq!(r.stdout, "rc=0 plain\n");
}

#[test]
fn local_flags_without_value() {
    let r =
        run("f() { local -r lr; local -x lx; local -i li; local -n ln; local -A lA; echo ok; }; f");
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn local_assoc_literal_non_key_words_are_ignored() {
    let r = run("f() { local -A lb=([k] plain); echo rc=$?; }; f");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn local_subscript_assignment_value() {
    // The attribute update after assignment looks up the literal name
    // `la[0]`, which never exists in env.
    let r = run("f() { local 'la[0]=x'; echo \"${la[0]}\"; }; f");
    assert_eq!(r.stdout, "x\n");
}

// ── let ─────────────────────────────────────────────────────────────

#[test]
fn let_without_args_is_usage_error() {
    let r = shell().exec("let").unwrap_err();
    assert!(
        matches!(r, rust_bash::RustBashError::Execution(ref msg) if msg == "let: usage: let arg [arg ...]"),
        "got: {r:?}"
    );
}

// ── type ────────────────────────────────────────────────────────────

#[test]
fn type_absolute_path() {
    let r = run("type /bin/cat");
    assert_eq!(r.stdout, "/bin/cat is /bin/cat\n");
}

#[test]
fn type_t_absolute_path() {
    let r = run("type -t /bin/cat");
    assert_eq!(r.stdout, "file\n");
}

#[test]
fn type_t_nonexecutable_absolute_path() {
    // `type -t` on a plain file falls back to the any-file PATH search.
    let r = run("touch /plainf; type -t /plainf");
    assert_eq!(r.stdout, "file\n");
}

#[test]
fn type_with_empty_path_component_searches_cwd() {
    let r = run("mkdir /w; touch /w/myf; cd /w; PATH=:; type -t myf");
    assert_eq!(r.stdout, "file\n");
}

#[test]
fn type_invalid_option() {
    let r = run("type -Z x");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "type: -Z: invalid option\n");
}

#[test]
fn type_with_only_flags_and_no_names() {
    let r = run("type -t; echo rc=$?");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn type_t_keyword() {
    let r = run("type -t if");
    assert_eq!(r.stdout, "keyword\n");
}

#[test]
fn type_function_with_subshell_body() {
    // Functions defined with `( )` bodies don't have a brace-group body,
    // so `type` falls back to a placeholder body.
    let r = run("fsb() ( echo hi ); type fsb");
    assert_eq!(r.stdout, "fsb is a function\nfsb () \n{ \n}\n");
}

#[test]
fn type_a_with_empty_path_component() {
    let r = run("mkdir /v; touch /v/tf; chmod +x /v/tf; cd /v; PATH=:/bin; type -a tf");
    assert_eq!(r.stdout, "tf is ./tf\n");
}

// ── command ─────────────────────────────────────────────────────────

#[test]
fn command_double_dash() {
    let r = run("command -- echo hi");
    assert_eq!(r.stdout, "hi\n");
}

#[test]
fn command_unknown_flag_becomes_command_name() {
    // Divergence: bash reports `command: -Z: invalid option` (exit 2);
    // rust-bash stops flag parsing and treats `-Z` as the command name.
    let r = run("command -Z echo hi");
    assert_eq!(r.exit_code, 127);
    assert_eq!(r.stderr, "-Z: command not found\n");
}

#[test]
fn command_with_path_containing_slash() {
    let r = run("command /bin/echo hi");
    assert_eq!(r.stdout, "hi\n");
}

#[test]
fn command_v_alias() {
    let r = run("alias ll='ls -l'; command -v ll");
    assert_eq!(r.stdout, "alias ll='ls -l'\n");
}

#[test]
fn command_big_v_special_builtin() {
    let r = run("command -V exit");
    assert_eq!(r.stdout, "exit is a shell builtin\n");
}

#[test]
fn command_big_v_registered_command() {
    let r = run("command -V grep");
    assert_eq!(r.stdout, "grep is /usr/bin/grep\n");
}

// ── builtin ─────────────────────────────────────────────────────────

#[test]
fn crafted_stub_for_unknown_target_reports_command_not_found() {
    // A hand-crafted "# built-in:" stub whose target is neither a builtin
    // nor a registered command reaches the not-found fallback.
    let mut sh = shell();
    sh.exec("printf '#!/bin/bash\\n# built-in: nosuchcmd\\n' > /bin/fake; chmod +x /bin/fake")
        .unwrap();
    let r = sh.exec("/bin/fake").unwrap();
    assert_eq!(r.exit_code, 127);
    assert_eq!(r.stderr, "nosuchcmd: command not found\n");
}

// ── execute_path_command ────────────────────────────────────────────

#[test]
fn executing_a_directory_is_permission_denied() {
    let r = run("mkdir /somedir; /somedir");
    assert_eq!(r.exit_code, 126);
    assert_eq!(r.stderr, "/somedir: Permission denied\n");
}

#[test]
fn executing_script_with_parse_error() {
    let r = run("printf 'if true; then' > /bad.sh; chmod +x /bad.sh; /bad.sh");
    assert_eq!(r.exit_code, 126);
    assert!(r.stderr.starts_with("/bad.sh: "), "stderr: {:?}", r.stderr);
}

// ── getopts ─────────────────────────────────────────────────────────

#[test]
fn getopts_usage_error() {
    let r = run("getopts ab");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "getopts: usage: getopts optstring name [arg ...]\n"
    );
}

#[test]
fn getopts_invalid_var_name_exhaustion() {
    let r = run("getopts a 1bad; echo rc=$?");
    assert_eq!(r.stdout, "rc=1\n");
}

#[test]
fn getopts_invalid_var_name_non_option_arg() {
    let r = run("getopts a 1bad foo; echo rc=$?");
    assert_eq!(r.stdout, "rc=1\n");
}

#[test]
fn getopts_sub_position_advance_past_bundled_flags() {
    let r =
        run("getopts ab o -ab; echo $o; getopts ab o -ab; echo $o; getopts ab o -ab; echo rc=$?");
    assert_eq!(r.stdout, "a\nb\nrc=1\n");
}

#[test]
fn getopts_sub_position_advance_with_shorter_bundle() {
    // A saved sub-position past the end of a shorter bundle advances
    // OPTIND and retries with the next argument.
    let r = run("getopts ab o -ab; OPTIND=1; getopts a o -a x; echo rc=$?");
    assert_eq!(r.stdout, "rc=1\n");
}

#[test]
fn getopts_reset_with_new_args_missing_arg() {
    // After the arg list changes and OPTIND is out of range, getopts
    // resets; the new spec needs an argument that isn't there.
    let r = run("getopts a o -a >/dev/null; getopts b: o -b; echo \"rc=$? o=$o\"");
    assert_eq!(r.stdout, "rc=1 o=?\n");
}

#[test]
fn getopts_reset_with_new_args_missing_arg_invalid_var_name() {
    let r = run("getopts a o -a >/dev/null; getopts b: 1bad -b; echo rc=$?");
    assert_eq!(r.stdout, "rc=1\n");
}

#[test]
fn getopts_silent_missing_arg_with_invalid_var_name() {
    let r = run("getopts :a: 1bad -a; echo \"rc=$? OPTARG=$OPTARG\"");
    assert_eq!(r.stdout, "rc=1 OPTARG=a\n");
}

#[test]
fn getopts_missing_arg_with_invalid_var_name() {
    let r = run("getopts a: 1bad -a; echo rc=$?");
    assert_eq!(r.stdout, "rc=1\n");
    assert_eq!(r.stderr, "getopts: option requires an argument -- 'a'\n");
}

#[test]
fn getopts_silent_invalid_option_with_invalid_var_name() {
    let r = run("getopts :a 1bad -z; echo \"rc=$? OPTARG=$OPTARG\"");
    assert_eq!(r.stdout, "rc=1 OPTARG=z\n");
}

#[test]
fn getopts_invalid_option_with_invalid_var_name() {
    let r = run("getopts a 1bad -z; echo rc=$?");
    assert_eq!(r.stdout, "rc=1\n");
    assert_eq!(r.stderr, "getopts: illegal option -- 'z'\n");
}

// ── mapfile ─────────────────────────────────────────────────────────

#[test]
fn mapfile_delimiter_attached() {
    let r = run("printf 'a,b,' > /d; mapfile -d, -t arr < /d; echo \"${#arr[@]} ${arr[1]}\"");
    assert_eq!(r.stdout, "2 b\n");
}

#[test]
fn mapfile_n_attached_count() {
    let r = run("printf 'a\\nb\\nc\\n' > /d; mapfile -n2 arr < /d; echo ${#arr[@]}");
    assert_eq!(r.stdout, "2\n");
}

#[test]
fn mapfile_n_without_count_defaults_to_zero() {
    let r = run("printf 'a\\nb\\n' > /d; mapfile -n < /d; echo \"rc=$? n=${#MAPFILE[@]}\"");
    assert_eq!(r.stdout, "rc=0 n=0\n");
}

#[test]
fn mapfile_s_attached_count() {
    let r = run("printf 'a\\nb\\nc\\n' > /d; mapfile -s1 arr < /d; echo ${arr[0]}");
    assert_eq!(r.stdout, "b\n");
}

#[test]
fn mapfile_s_without_count_defaults_to_zero() {
    let r = run("printf 'a\\nb\\n' > /d; mapfile -s < /d; echo ${MAPFILE[0]}");
    assert_eq!(r.stdout, "a\n");
}

#[test]
fn mapfile_callback_options_consume_values() {
    // -C callback / -c quantum are accepted but the callback is not
    // invoked (pinned).
    let r = run("printf 'a\\nb\\nc\\n' > /d; mapfile -C true -c 2 arr < /d; echo ${#arr[@]}");
    assert_eq!(r.stdout, "3\n");
}

#[test]
fn mapfile_o_attached_origin() {
    let r = run("printf 'a\\n' > /d; mapfile -O5 arr < /d; echo ${arr[5]}");
    assert_eq!(r.stdout, "a\n");
}

#[test]
fn mapfile_o_without_origin_defaults_to_zero() {
    let r = run("printf 'a\\n' > /d; mapfile -O < /d; echo ${MAPFILE[0]}");
    assert_eq!(r.stdout, "a\n");
}

#[test]
fn mapfile_invalid_option() {
    let r = run("mapfile -Z arr");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "mapfile: -Z: invalid option\n");
}

#[test]
fn mapfile_origin_with_existing_scalar_replaces_it() {
    let r = run("ms=scalar; printf 'a\\n' > /d; mapfile -O 2 ms < /d; echo ${ms[2]}");
    assert_eq!(r.stdout, "a\n");
}

#[test]
fn mapfile_origin_with_fresh_variable() {
    let r = run("printf 'a\\n' > /d; mapfile -O 2 mnew < /d; echo ${mnew[2]}");
    assert_eq!(r.stdout, "a\n");
}

#[test]
fn mapfile_propagates_array_limit() {
    let mut sh = RustBashBuilder::new()
        .max_array_elements(1)
        .build()
        .unwrap();
    let err = sh.exec("printf 'a\\nb\\n' | mapfile arr").unwrap_err();
    assert!(
        matches!(
            err,
            rust_bash::RustBashError::LimitExceeded {
                limit_name: "max_array_elements",
                ..
            }
        ),
        "got: {err:?}"
    );
}

#[test]
fn mapfile_last_line_without_trailing_delimiter() {
    let r = run("printf 'a\\nb' > /d; mapfile -t arr < /d; echo \"${#arr[@]} ${arr[1]}\"");
    assert_eq!(r.stdout, "2 b\n");
}

// ── pushd / popd / dirs ─────────────────────────────────────────────

#[test]
fn pushd_no_args_with_empty_stack() {
    let r = run("pushd");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "pushd: no other directory\n");
}

#[test]
fn pushd_no_args_swaps_top_two() {
    let r = run("mkdir /a /b; cd /a; pushd /b >/dev/null; pushd >/dev/null; pwd");
    assert_eq!(r.stdout, "/a\n");
}

#[test]
fn pushd_swap_restores_stack_when_cd_fails() {
    let r = run("mkdir /d; pushd /d >/dev/null; pushd / >/dev/null; rmdir /d; pushd; echo rc=$?");
    assert_eq!(r.stdout, "rc=1\n");
    assert_eq!(r.stderr, "cd: /d: No such file or directory\n");
}

#[test]
fn pushd_dash_goes_to_oldpwd() {
    let r = run("mkdir /a /b; cd /a; pushd /b >/dev/null; pushd - >/dev/null; pwd");
    assert_eq!(r.stdout, "/a\n");
}

#[test]
fn pushd_rotate_plus_n() {
    let r = run("mkdir /a /b /c; cd /a; pushd /b >/dev/null; pushd /c >/dev/null; pushd +1; pwd");
    assert_eq!(r.stdout, "/b /a /c\n/b\n");
}

#[test]
fn pushd_rotate_minus_n() {
    let r = run("mkdir /a /b /c; cd /a; pushd /b >/dev/null; pushd /c >/dev/null; pushd -1; pwd");
    assert_eq!(r.stdout, "/a /c /b\n/a\n");
}

#[test]
fn pushd_rotate_out_of_range() {
    let r = run("mkdir /a; cd /a; pushd +9");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "pushd: +9: directory stack index out of range\n");
}

#[test]
fn pushd_to_nonexistent_directory() {
    let r = run("pushd /nonexistent");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "cd: /nonexistent: No such file or directory\n");
}

#[test]
fn popd_dashdash_with_empty_stack() {
    let r = run("popd --");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "popd: directory stack empty\n");
}

#[test]
fn popd_dashdash_restores_entry_when_cd_fails() {
    let r = run(
        "mkdir /d; pushd /d >/dev/null; pushd / >/dev/null; rmdir /d; popd --; echo rc=$?; dirs",
    );
    assert_eq!(r.stdout, "rc=1\n/ /d /\n");
}

#[test]
fn popd_plus_n_removes_stack_entry() {
    let r = run("mkdir /a /b /c; cd /a; pushd /b >/dev/null; pushd /c >/dev/null; popd +1");
    assert_eq!(r.stdout, "/c /a\n");
}

#[test]
fn popd_plus_zero_replaces_cwd() {
    let r = run("mkdir /a /b; cd /a; pushd /b >/dev/null; popd +0; pwd");
    assert_eq!(r.stdout, "/a\n/a\n");
}

#[test]
fn popd_minus_n_removes_from_other_end() {
    let r = run("mkdir /a /b /c; cd /a; pushd /b >/dev/null; pushd /c >/dev/null; popd -0");
    assert_eq!(r.stdout, "/c /b\n");
}

#[test]
fn popd_index_out_of_range() {
    let r = run("mkdir /a; cd /a; popd +5");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "popd: +5: directory stack index out of range\n");
}

#[test]
fn popd_restores_entry_when_cd_fails() {
    let r = run(
        "mkdir /d2; pushd /d2 >/dev/null; pushd / >/dev/null; rmdir /d2; popd; echo rc=$?; dirs",
    );
    assert_eq!(r.stdout, "rc=1\n/ /d2 /\n");
}

#[test]
fn dirs_bare_dash_is_invalid() {
    let r = run("dirs -");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "dirs: -: invalid option\n");
}

#[test]
fn dirs_invalid_option() {
    let r = run("dirs -Z");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "dirs: -Z: invalid option\n");
}

#[test]
fn dirs_plus_n_is_accepted_and_ignored() {
    // Divergence: bash prints only the +N-th stack entry; rust-bash
    // accepts the syntax but prints the whole stack.
    let r = run("mkdir /a /b; cd /a; pushd /b >/dev/null; dirs +0");
    assert_eq!(r.stdout, "/b /a\n");
}

#[test]
fn dirs_shows_home_as_tilde() {
    let r = run("mkdir -p /home/user/sub; cd /; pushd /home/user/sub >/dev/null; dirs -v; dirs -p");
    assert_eq!(r.stdout, " 0  ~/sub\n 1  /\n~/sub\n/\n");
}

// ── hash / alias ────────────────────────────────────────────────────

#[test]
fn hash_ignores_other_flags() {
    let r = run("hash -t; echo rc=$?");
    assert_eq!(r.stdout, "rc=0\n");
}

#[test]
fn alias_p_prints_all_aliases() {
    let r = run("alias a1='x'; alias -p");
    assert_eq!(r.stdout, "alias a1='x'\n");
}

// ── printf ──────────────────────────────────────────────────────────

#[test]
fn printf_double_dash_without_format_is_usage_error() {
    let r = run("printf --");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "printf: usage: printf [-v var] format [arguments]\n"
    );
}

#[test]
fn printf_without_args_is_usage_error() {
    let r = run("printf");
    assert_eq!(r.exit_code, 2);
    assert_eq!(
        r.stderr,
        "printf: usage: printf [-v var] format [arguments]\n"
    );
}

// ── sh / bash ───────────────────────────────────────────────────────

#[test]
fn sh_rcfile_without_argument() {
    let r = run("sh --rcfile");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "sh: --rcfile: option requires an argument\n");
}

#[test]
fn sh_plus_i_disables_interactive() {
    let r = run("sh +i -c 'echo ok'");
    assert_eq!(r.stdout, "ok\n");
}

#[test]
fn sh_o_without_option_name() {
    let r = run("sh -o");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "sh: -o: option requires an argument\n");
}

#[test]
fn sh_plus_o_without_option_name() {
    let r = run("sh +o");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "sh: +o: option requires an argument\n");
}

#[test]
fn sh_o_invalid_option_name() {
    let r = run("sh -o badname -c :");
    assert_eq!(r.exit_code, 2);
    assert_eq!(r.stderr, "sh: badname: invalid option name\n");
}

#[test]
fn sh_missing_script_file() {
    let r = run("sh /nonexistent");
    assert_eq!(r.exit_code, 127);
    assert!(
        r.stderr.starts_with("sh: /nonexistent: "),
        "stderr: {:?}",
        r.stderr
    );
}

// ── help ────────────────────────────────────────────────────────────

#[test]
fn help_for_registered_command() {
    let r = run("help grep");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stderr, "");
    assert!(
        r.stdout
            .starts_with("Usage: grep [OPTIONS] PATTERN [FILE ...]\n"),
        "stdout: {:?}",
        r.stdout
    );
    assert!(
        r.stdout.contains("-i, --ignore-case"),
        "stdout: {:?}",
        r.stdout
    );
    assert!(r.stdout.contains("Flag support:"), "stdout: {:?}", r.stdout);
}

#[test]
fn help_for_unknown_topic() {
    let r = run("help nosuchtopic");
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.stderr, "help: no help topics match 'nosuchtopic'\n");
}
