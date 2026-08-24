//! Coverage tests for src/interpreter/expansion.rs: tilde expansion forms,
//! ANSI-C and prompt escape sequences, the `@Q`/`@E`/`@P`/`@A`/`@a`
//! transforms, `SHELLOPTS`/`BASHOPTS`/`$-` computation, and pattern
//! replacement-string features (tilde, escapes, `$` references).
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

// ── Tilde expansion ─────────────────────────────────────────────────

#[test]
fn tilde_inside_double_quotes_stays_literal() {
    let (out, err, code) = run(r#"echo "~""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("~\n", "", 0));
}

#[test]
fn tilde_forms_inside_double_quotes_stay_literal() {
    // Each form must be at word start for the parser to recognize it as a
    // tilde expression; inside double quotes it is preserved literally.
    let (out, err, code) =
        run(r#"echo "~+"; echo "~-"; echo "~root"; echo "~+2"; echo "~2"; echo "~-2""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("~+\n~-\n~root\n~+2\n~2\n~-2\n", "", 0)
    );
}

#[test]
fn tilde_forms_in_double_quoted_default_words_stay_literal() {
    // Default words are re-parsed as independent words, so a leading tilde
    // form is recognized there, too — and preserved literally in DQ context.
    let (out, err, code) =
        run(r#"echo "${u:-~+} ${u:-~-} ${u:-~root} ${u:-~+2} ${u:-~2} ${u:-~-2}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("~+ ~- ~root ~+2 ~2 ~-2\n", "", 0)
    );
}

#[test]
fn tilde_plus_and_minus_expand_pwd_and_oldpwd() {
    // Default environment: PWD=/ (the cwd), OLDPWD="".
    let (out, err, code) = run("echo ~+ ~-");
    assert_eq!((out.as_str(), err.as_str(), code), ("/ \n", "", 0));
}

#[test]
fn tilde_dir_stack_forms_expand_to_bare_tilde() {
    // DIVERGENCE?: real bash resolves ~N/~-N via the directory stack; the
    // sandbox has no dir stack tilde support and collapses these to "~".
    let (out, err, code) = run("echo ~2 ~-2");
    assert_eq!((out.as_str(), err.as_str(), code), ("~ ~\n", "", 0));
}

#[test]
fn assignment_like_words_expand_tilde_prefixes() {
    let mut sh = RustBashBuilder::new().cwd("/work").build().unwrap();
    let r = sh.exec("v=~+/x; w=~-/y; echo \"$v|$w\"").unwrap();
    assert_eq!(
        (r.stdout.as_str(), r.stderr.as_str(), r.exit_code),
        ("/work/x|/y\n", "", 0)
    );
}

// ── ANSI-C escape sequences ─────────────────────────────────────────

#[test]
fn ansic_simple_escapes_produce_control_bytes() {
    let (out, err, code) = run(r#"echo $'\a\b\f\v\e\E'"#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("\u{7}\u{8}\u{c}\u{b}\u{1b}\u{1b}\n", "", 0)
    );
}

#[test]
fn ansic_hex_escape_without_digits_is_literal() {
    let (out, err, code) = run(r#"echo $'\x'"#);
    assert_eq!((out.as_str(), err.as_str(), code), ("\\x\n", "", 0));
}

#[test]
fn ansic_big_unicode_escape_without_digits_is_literal() {
    let (out, err, code) = run(r#"echo $'\U'"#);
    assert_eq!((out.as_str(), err.as_str(), code), ("\\U\n", "", 0));
}

#[test]
fn ansic_ctrl_escape_at_end_of_string_is_literal() {
    let (out, err, code) = run(r#"echo $'\c'"#);
    assert_eq!((out.as_str(), err.as_str(), code), ("\\c\n", "", 0));
}

// ── @Q (shell quoting) ──────────────────────────────────────────────

#[test]
fn quote_transform_uses_dollar_quote_notation_for_control_chars() {
    let (out, err, code) = run(r#"x=$'\\\t\r\a\b\f\v\e'; echo "${x@Q}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("$'\\\\\\t\\r\\a\\b\\f\\v\\E'\n", "", 0)
    );
}

// ── @P (prompt expansion) ───────────────────────────────────────────

#[test]
fn prompt_transform_expands_user_host_and_cwd_sequences() {
    // Default environment: USER=user, HOSTNAME=rust-bash, HOME=/home/user,
    // cwd=/, shell name "rust-bash".
    let (out, err, code) =
        run(r#"p='[\u][\h][\H][\w][\W][\s][\v][\$][\[][\]][\\][\z]'; echo "${p@P}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        (
            "[user][rust-bash][rust-bash][/][/][rust-bash][5.0][$][][][\\][\\z]\n",
            "",
            0
        )
    );
}

#[test]
fn prompt_transform_expands_fixed_clock_sequences() {
    // DIVERGENCE?: real bash uses the wall clock; the sandbox pins all
    // date/time prompt escapes to fixed strings for determinism.
    let (out, err, code) = run(r#"p='[\d][\t][\T][\@][\A]'; echo "${p@P}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("[Mon Jan 01][00:00:00][12:00:00][12:00 AM][00:00]\n", "", 0)
    );
}

#[test]
fn prompt_transform_expands_control_sequences() {
    let (out, err, code) = run(r#"p='x\ny\rz\aw\ev'; echo "${p@P}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("x\ny\rz\u{7}w\u{1b}v\n", "", 0)
    );
}

#[test]
fn prompt_transform_command_count_sequence() {
    let mut sh = shell();
    let r = sh.exec(r#"p='\#'; echo "${p@P}""#).unwrap();
    // The assignment does not count as a command, so the echo sees a
    // command count of 0.
    assert_eq!(r.stdout, "0\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn prompt_transform_tilde_collapses_home_prefix() {
    let mut sh = RustBashBuilder::new()
        .cwd("/home/user/sub")
        .build()
        .unwrap();
    let r = sh.exec(r#"p='[\w][\W]'; echo "${p@P}""#).unwrap();
    assert_eq!(r.stdout, "[~/sub][sub]\n");
    assert_eq!(r.exit_code, 0);
}

// ── @A (assignment form) ────────────────────────────────────────────

#[test]
fn assignment_transform_formats_scalars_and_flags() {
    let (out, err, code) = run(
        r#"s=plain; declare -i i=5; declare -l l=AB; declare -u u=ab; declare -r r=v; declare -x x=v
echo "${s@A}|${i@A}|${l@A}|${u@A}|${r@A}|${x@A}""#,
    );
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        (
            "s='plain'|declare -i i='5'|declare -l l='ab'|declare -u u='AB'|declare -r r='v'|declare -x x='v'\n",
            "",
            0
        )
    );
}

#[test]
fn assignment_transform_formats_indexed_and_assoc_arrays() {
    let (out, err, code) = run(r#"a=(x y); declare -A A=([k]=v); echo "${a@A}"; echo "${A@A}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        (
            "declare -a a=([0]=\"x\" [1]=\"y\")\ndeclare -A A=([k]=\"v\")\n",
            "",
            0
        )
    );
}

#[test]
fn assignment_transform_of_nameref_to_missing_variable_is_empty() {
    let (out, err, code) = run(r#"declare -n nr=missing; echo "[${nr@A}]|[${nr@a}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[]|[]\n", "", 0));
}

#[test]
fn assignment_transform_through_chained_nameref_resolves_to_final_target() {
    // a → b → c: nameref resolution follows the whole chain, so the formatted
    // assignment describes the final target c (with no attributes).
    let (out, err, code) = run(r#"declare -n a=b; declare -n b=c; c=v; echo "${a@A}|[${a@a}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("c='v'|[]\n", "", 0));
}

#[test]
fn assignment_transform_of_circular_nameref_shows_nameref_flag() {
    // Nameref resolution gives up on cycles and returns the nameref itself,
    // so its NAMEREF attribute shows up in the formatted output.
    let (out, err, code) = run(r#"declare -n a=b; declare -n b=a; echo "${a@A}|${a@a}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("declare -n a='b'|n\n", "", 0)
    );
}

// ── @a (attribute flags) ────────────────────────────────────────────

#[test]
fn attribute_flags_transform_reports_declared_attributes() {
    let (out, err, code) = run(
        r#"a=(x); declare -A B; declare -l l=v; declare -u u=v; s=plain
echo "${a@a}|${B@a}|${l@a}|${u@a}|[${s@a}]""#,
    );
    assert_eq!((out.as_str(), err.as_str(), code), ("a|A|l|u|[]\n", "", 0));
}

// ── Transforms over unset/special parameters ────────────────────────

#[test]
fn transforms_of_unset_or_dynamic_scalars() {
    let (out, err, code) = run(
        r#"echo "[${FUNCNAME@Q}]|[${missing@Q}]|${LINENO@Q}"; f() { local v; echo "[${v@Q}]"; }; f"#,
    );
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("[]|[]|'1'\n[]\n", "", 0)
    );
}

#[test]
fn transforms_via_empty_or_positional_indirect_targets() {
    let (out, err, code) =
        run(r#"r=; echo "[${!r@Q}]|[${!r@A}]"; r=1; set -- v; echo "[${!r@A}]|[${2@A}]|[${1@A}]""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("[]|[]\n[]|[]|[]\n", "", 0)
    );
}

#[test]
fn quote_transform_over_assoc_array_elements() {
    // DIVERGENCE?: bash does not reverse assoc-array elements for
    // ${arr[@]@Q}; the reversal exists to counteract a B-tree ordering
    // assumption elsewhere. Via an indirect `${!r}` the expansion collapses
    // to a single joined word instead of one quoted word per element.
    let mut sh = shell();
    let r = sh
        .exec(r#"declare -A A=([k1]=v1 [k2]=v2); echo "${A[@]@Q}"; r='A[@]'; echo "${!r@Q}""#)
        .unwrap();
    assert_eq!(r.stdout, "'v2' 'v1'\n'v1 v2'\n");
    assert_eq!(r.exit_code, 0);
}

// ── SHELLOPTS / BASHOPTS / $- ───────────────────────────────────────

#[test]
fn shellopts_reflects_enabled_set_options() {
    let (out, err, code) = run(r#"set -a -C -f -u -o pipefail -o posix -o vi; echo "$SHELLOPTS""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        (
            "allexport:braceexpand:hashall:noclobber:noglob:nounset:pipefail:posix:vi\n",
            "",
            0
        )
    );
}

#[test]
fn shellopts_reflects_emacs_editing_mode() {
    // emacs and vi are mutually exclusive; enabling vi clears emacs.
    let (out, err, code) = run(r#"set -o emacs; echo "$SHELLOPTS""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("braceexpand:emacs:hashall\n", "", 0)
    );
}

#[test]
fn shellopts_reflects_verbose() {
    let mut sh = shell();
    let r = sh.exec("set -v; echo \"$SHELLOPTS\"").unwrap();
    assert_eq!(r.stdout, "braceexpand:hashall:verbose\n");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn bashopts_reflects_enabled_shopt_options() {
    let mut sh = shell();
    let r = sh
        .exec(
            "shopt -s assoc_expand_once autocd cdable_vars cdspell checkhash checkjobs \
             direxpand dirspell dotglob execfail expand_aliases extdebug failglob gnu_errfmt \
             globstar histappend histreedit histverify huponexit inherit_errexit lastpipe \
             lithist localvar_inherit localvar_unset login_shell mailwarn no_empty_cmd_completion \
             nocaseglob nocasematch progcomp_alias shift_verbose varredir_close xpg_echo; \
             echo \"$BASHOPTS\"",
        )
        .unwrap();
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    for opt in [
        "assoc_expand_once",
        "autocd",
        "cdable_vars",
        "cdspell",
        "checkhash",
        "checkjobs",
        "direxpand",
        "dirspell",
        "dotglob",
        "execfail",
        "expand_aliases",
        "extdebug",
        "failglob",
        "gnu_errfmt",
        "globstar",
        "histappend",
        "histreedit",
        "histverify",
        "huponexit",
        "inherit_errexit",
        "lastpipe",
        "lithist",
        "localvar_inherit",
        "localvar_unset",
        "login_shell",
        "mailwarn",
        "no_empty_cmd_completion",
        "nocaseglob",
        "nocasematch",
        "progcomp_alias",
        "shift_verbose",
        "varredir_close",
        "xpg_echo",
    ] {
        assert!(
            r.stdout.trim_end().split(':').any(|o| o == opt),
            "BASHOPTS missing {opt}: {}",
            r.stdout
        );
    }
}

#[test]
fn dollar_dash_reflects_option_flags() {
    let (out, err, code) = run(r#"echo $-; set -a; echo $-"#);
    assert_eq!((out.as_str(), err.as_str(), code), ("hBs\nhaBs\n", "", 0));
}

// ── Pattern replacement strings ─────────────────────────────────────

#[test]
fn replacement_leading_tilde_expands_home_pwd_oldpwd_and_root() {
    let (out, err, code) = run(
        r#"v=axb; echo "${v/x/~/s}"; echo "${v/x/~+}"; echo "${v/x/~-}"; echo "${v/x/~root}"; echo "${v/x/~zz}""#,
    );
    // Default environment: HOME=/home/user, PWD=/, OLDPWD="".
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("a/home/user/sb\na/b\nab\na/rootb\na~zzb\n", "", 0)
    );
}

#[test]
fn replacement_backslash_before_ordinary_char_is_kept() {
    let (out, err, code) = run(r#"v=axb; echo "${v/x/\q}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("a\\qb\n", "", 0));
}

#[test]
fn replacement_ansic_quotes_expand_escapes() {
    let (out, err, code) = run(r#"v=axb; echo "${v/x/$'\t'}"; echo "${v/x/$'\z'}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("a\tb\na\\zb\n", "", 0));
}

#[test]
fn replacement_double_quoted_backslash_rules() {
    // DIVERGENCE?: in real bash the escaped `\$` stays a literal dollar and
    // `$q` is NOT expanded afterwards; here the unescaped replacement text is
    // re-expanded, so `$q` (unset) vanishes. A backslash before an ordinary
    // char is preserved, matching bash.
    let (out, err, code) = run(r#"v=axb; echo "${v/x/"p\$q"}"; echo "${v/x/"p\zq"}""#);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("apb\nap\\zqb\n", "", 0)
    );
}

#[test]
fn replacement_parameter_references_expand() {
    let (out, err, code) = run(
        r#"r=R; v=axb; set -- p q; echo "${v/x/$r}"; echo "${v/x/${r}}"; echo "${v/x/$2}"; echo "${v/x/$0}""#,
    );
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("aRb\naRb\naqb\narust-bashb\n", "", 0)
    );
}

#[test]
fn replacement_special_parameter_references_expand() {
    let (out, err, code) = run(
        r#"v=axb; set -- p q; echo "${v/x/$#}"; echo "${v/x/$?}"; echo "${v/x/$@}"; echo "${v/x/$*}"; echo "${v/x/$-}"; echo "${v/x/$!}""#,
    );
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("a2b\na0b\nap qb\nap qb\nab\nab\n", "", 0)
    );
}

#[test]
fn replacement_process_id_reference_is_a_fixed_value() {
    // DIVERGENCE?: real bash substitutes the shell's PID for $$; the sandbox
    // replacement-string path hardcodes "1".
    let (out, err, code) = run(r#"v=axb; echo "${v/x/$$}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("a1b\n", "", 0));
}

#[test]
fn replacement_lone_dollar_is_literal() {
    let (out, err, code) = run(r#"v=axb; echo "${v/x/$}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("a$b\n", "", 0));
}

// ── Ambiguous ternary slice rewriting ───────────────────────────────

#[test]
fn substring_with_ternary_like_offset_containing_brackets() {
    // `${x:a[0]?1:2:3}` is ambiguous between `${x:offset:length}` and a ternary
    // default; the rewriter wraps the offset as arithmetic `$((a[0]?1:2))`.
    let (out, err, code) = run(r#"a=(7); x=abcdef; echo "${x:a[0]?1:2:3}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("bcd\n", "", 0));
}
