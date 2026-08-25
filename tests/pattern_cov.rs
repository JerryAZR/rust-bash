//! Coverage tests for src/interpreter/pattern.rs: UTF-8 character handling,
//! path-aware matching (GLOBIGNORE), LC_ALL=C byte mode, character-class
//! corners (negation, escapes, POSIX named classes), and extglob matching
//! corners. All tests drive the public `RustBash` API and pin exact
//! stdout/stderr/exit codes. Suspected divergences from real bash are marked
//! with `DIVERGENCE?` comments.
//!
//! Note: case-statement patterns must be UNQUOTED to act as globs — quoting
//! a pattern makes it match literally (bash behavior, mirrored here).

use rust_bash::{RustBash, RustBashBuilder};
use std::collections::HashMap;

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

// ── UTF-8 character lengths ─────────────────────────────────────────

#[test]
fn question_wildcard_matches_one_full_multibyte_character() {
    // `?` must consume a whole UTF-8 character: 2-, 3-, and 4-byte forms.
    let (out, err, code) = run(
        "case é in ?) echo two;; esac; case 世 in ?) echo three;; esac; case \u{1F600} in ?) echo four;; esac",
    );
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("two\nthree\nfour\n", "", 0)
    );
}

#[test]
fn first_match_skips_non_char_boundary_positions() {
    // The match scan must skip byte offsets inside the 2-byte `é`.
    let (out, err, code) = run("v=élan; echo ${v/l/L}");
    assert_eq!((out.as_str(), err.as_str(), code), ("éLan\n", "", 0));
}

// ── Path-aware matching (GLOBIGNORE) ────────────────────────────────

#[test]
fn globignore_question_mark_does_not_match_slash() {
    let files: HashMap<String, Vec<u8>> =
        [("/a/b".to_string(), b"x".to_vec())].into_iter().collect();
    let mut sh = RustBashBuilder::new().files(files).build().unwrap();
    // `?` in a GLOBIGNORE pattern must not match the `/` in `a/b`, so the
    // file survives filtering.
    let r = sh.exec("GLOBIGNORE='a?b'; echo a/*").unwrap();
    assert_eq!(
        (r.stdout.as_str(), r.stderr.as_str(), r.exit_code),
        ("a/b\n", "", 0)
    );
}

// ── LC_ALL=C byte mode ──────────────────────────────────────────────

#[test]
fn byte_locale_star_backtracks_by_single_bytes() {
    let (out, err, code) = run("LC_ALL=C; v=aaab; echo ${v/a*b/X}");
    assert_eq!((out.as_str(), err.as_str(), code), ("X\n", "", 0));
}

#[test]
fn byte_locale_extglob_pattern_matches_utf8_text() {
    let (out, err, code) = run("LC_ALL=C; shopt -s extglob; v=foobar; echo ${v/@(foo|baz)/X}");
    assert_eq!((out.as_str(), err.as_str(), code), ("Xbar\n", "", 0));
}

#[test]
fn byte_locale_first_match_replacement_and_no_match() {
    let (out, err, code) = run("LC_ALL=C; v=hello; echo ${v/l/L}; echo ${v/z/L}");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("heLlo\nhello\n", "", 0)
    );
}

// ── Character classes ───────────────────────────────────────────────

#[test]
fn nocasematch_applies_inside_class_ranges() {
    let (out, err, code) =
        run("shopt -s nocasematch; case G in [a-z]) echo yes;; *) echo no;; esac");
    assert_eq!((out.as_str(), err.as_str(), code), ("yes\n", "", 0));
}

#[test]
fn negated_class_with_escaped_bracket_member() {
    // `[^]\]]` is a negated class whose only member is `]`.
    let (out, err, code) = run(
        r"case x in [^]\]]) echo notbr;; *) echo no;; esac; case ']' in [^]\]]) echo bad;; *) echo skip;; esac",
    );
    assert_eq!((out.as_str(), err.as_str(), code), ("notbr\nskip\n", "", 0));
}

#[test]
fn caret_bracket_class_without_extra_members_is_not_a_negation() {
    // DIVERGENCE? Real bash treats `[^]]` as a negated class matching
    // anything except `]` (so `x` would match). rust-bash only treats `[^]`
    // as negation when further members follow, so it does not match here.
    let (out, err, code) = run("case x in [^]]) echo yes;; *) echo no;; esac");
    assert_eq!((out.as_str(), err.as_str(), code), ("no\n", "", 0));
}

#[test]
fn posix_named_classes_match_their_members() {
    let (out, err, code) = run("case 5 in [[:alnum:]]) echo alnum;; esac; \
         case A in [[:upper:]]) echo upper;; esac; \
         case a in [[:lower:]]) echo lower;; esac; \
         case ' ' in [[:space:]]) echo space;; esac; \
         case ' ' in [[:blank:]]) echo blank;; esac; \
         case ' ' in [[:print:]]) echo print;; esac; \
         case '!' in [[:graph:]]) echo graph;; esac; \
         case $'\\x01' in [[:cntrl:]]) echo cntrl;; esac; \
         case f in [[:xdigit:]]) echo xdigit;; esac; \
         case z in [[:ascii:]]) echo ascii;; esac");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        (
            "alnum\nupper\nlower\nspace\nblank\nprint\ngraph\ncntrl\nxdigit\nascii\n",
            "",
            0
        )
    );
}

#[test]
fn unknown_posix_named_class_never_matches() {
    let (out, err, code) = run("case q in [[:wqert:]]) echo bad;; *) echo nomatch;; esac");
    assert_eq!((out.as_str(), err.as_str(), code), ("nomatch\n", "", 0));
}

#[test]
fn posix_class_with_non_alphanumeric_name_falls_back_to_literals() {
    // `-` inside `[: :]` aborts the POSIX class scan; the class then matches
    // its literal bytes: `[` (from the class) followed by a literal `]`.
    // DIVERGENCE? Unverified against real bash, which may reject the whole
    // bracket expression (and thus not match `[]`) on the invalid class name.
    let (out, err, code) = run("case '[]' in [[:a-1:]]) echo hit;; *) echo miss;; esac");
    assert_eq!((out.as_str(), err.as_str(), code), ("hit\n", "", 0));
}

#[test]
fn unterminated_posix_class_falls_back_to_literal_bracket() {
    // No closing `:]` — the bracket expression is invalid and the leading
    // `[` matches literally, so the pattern matches its own literal text.
    let (out, err, code) = run("case '[[:abc' in [[:abc) echo hit;; *) echo miss;; esac");
    assert_eq!((out.as_str(), err.as_str(), code), ("hit\n", "", 0));
}

// ── Extglob corners ─────────────────────────────────────────────────

#[test]
fn extglob_pattern_may_contain_character_classes() {
    let (out, err, code) = run("shopt -s extglob; \
         case ab in @(a)[bc]) echo hit;; *) echo miss;; esac; \
         case ax in @(a)[bc]) echo bad;; *) echo miss2;; esac; \
         case 'a[' in @(a)[) echo uncl;; *) echo miss3;; esac; \
         case ax in @(a)[) echo bad3;; *) echo miss4;; esac; \
         case a in @(a)[bc]) echo bad2;; *) echo short;; esac");
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("hit\nmiss2\nuncl\nmiss4\nshort\n", "", 0)
    );
}

#[test]
fn extglob_star_runs_stop_before_an_extglob_group() {
    // `@(a)**b`: the star run collapses and `b` anchors the end.
    // `@(a)**(b)`: the star run stops before the `*(b)` extglob group.
    let (out, err, code) = run("shopt -s extglob; \
         case ab in @(a)**b) echo hit;; *) echo miss;; esac; \
         case ab in @(a)**(b)) echo hit2;; *) echo miss2;; esac");
    assert_eq!((out.as_str(), err.as_str(), code), ("hit\nhit2\n", "", 0));
}

#[test]
fn extglob_operator_introduced_by_expansion_without_close_is_literal() {
    // The parser rejects unbalanced `@(` written literally, but a pattern
    // built by expansion can contain one; it then matches literally.
    let (out, err, code) = run("shopt -s extglob; p='@('; v='ax@('; echo \"${v##@(a)x$p}\"");
    assert_eq!((out.as_str(), err.as_str(), code), ("\n", "", 0));

    let (out, err, code) =
        run("shopt -s extglob; p='@('; case 'ax@(' in @(a)x\"$p\") echo hit;; *) echo miss;; esac");
    assert_eq!((out.as_str(), err.as_str(), code), ("hit\n", "", 0));
}

#[test]
fn extglob_nesting_beyond_depth_limit_fails_to_match() {
    // DIVERGENCE? Real bash has no nesting-depth limit and would match `a`
    // here; rust-bash caps extglob recursion at depth 64 as a runaway-
    // recursion guard, so the pattern fails to match.
    let pattern = format!("{}a{}", "+(".repeat(70), ")".repeat(70));
    let script = format!("shopt -s extglob; case a in {pattern}) echo m;; *) echo nom;; esac");
    let (out, err, code) = run(&script);
    assert_eq!((out.as_str(), err.as_str(), code), ("nom\n", "", 0));
}
