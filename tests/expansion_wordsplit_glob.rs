//! Coverage tests for src/interpreter/expansion.rs: IFS word splitting edge
//! cases, `$@`/`$*` empty-expansion semantics, glob expansion with
//! GLOBIGNORE/failglob, and command substitution corner cases.
//!
//! All tests drive the public `RustBash` API and pin exact stdout/stderr/exit
//! codes. Where the pinned behavior is suspected to diverge from real bash,
//! the expectation carries a `DIVERGENCE?` comment.

use rust_bash::{RustBash, RustBashBuilder};
use std::collections::HashMap;

fn shell() -> RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn shell_with_files(files: &[(&str, &str)]) -> RustBash {
    let map: HashMap<String, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (p.to_string(), c.as_bytes().to_vec()))
        .collect();
    RustBashBuilder::new().files(map).build().unwrap()
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

// ── Unquoted $* / ${arr[*]} with IFS="" ─────────────────────────────

#[test]
fn unquoted_star_with_empty_ifs_and_no_positionals_vanishes() {
    // Unquoted $* with IFS="" behaves like $@; with zero positional params it
    // produces no word at all, so surrounding literal text is unaffected.
    let (out, err, code) = run(r#"IFS=""; set --; echo a${*}b"#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

#[test]
fn unquoted_array_star_with_empty_ifs_and_no_elements_vanishes() {
    let (out, err, code) = run(r#"IFS=""; a=(); echo a${a[*]}b"#);
    assert_eq!((out.as_str(), err.as_str(), code), ("ab\n", "", 0));
}

// ── ${arr[*]} joining with unset IFS ────────────────────────────────

#[test]
fn quoted_array_star_joins_with_space_when_ifs_unset() {
    let (out, err, code) = run(r#"unset IFS; a=(x y); echo "${a[*]}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("x y\n", "", 0));
}

#[test]
fn vectorized_suffix_strip_on_star_joins_with_space_when_ifs_unset() {
    let (out, err, code) = run(r#"unset IFS; a=(x1 y2); echo "${a[*]%2}""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("x1 y\n", "", 0));
}

// ── Synthetic empty fields from unquoted ${arr[@]} ──────────────────

#[test]
fn unquoted_array_at_preserves_empty_element_with_nonws_ifs() {
    // IFS=: is a non-whitespace delimiter, so the empty element of a survives
    // splitting as a real empty field.
    let (out, err, code) = run(r#"IFS=:; a=("" b); set -- ${a[@]}; echo "$#|$1|$2""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("2||b\n", "", 0));
}

#[test]
fn vectorized_replace_producing_empty_element_yields_empty_field_with_nonws_ifs() {
    let (out, err, code) = run(r#"IFS=:; a=(x y); set -- ${a[@]/x/}; echo "$#|$1|$2""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("2||y\n", "", 0));
}

// ── Quoted anchors in IFS splitting ─────────────────────────────────

#[test]
fn quoted_empty_segment_between_ifs_only_expansions_anchors_one_empty_field() {
    // $x""$y with x=y=" " — all unquoted content is IFS whitespace and splits
    // away, but the quoted "" anchors the word to exactly one empty field.
    let (out, err, code) = run(r#"x=" "; y=" "; set -- $x""$y; echo "$#""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("1\n", "", 0));
}

// ── Glob: GLOBIGNORE + failglob ─────────────────────────────────────

#[test]
fn globignore_filtering_all_matches_with_failglob_is_an_error() {
    let mut sh = shell_with_files(&[("/d/a.txt", "a"), ("/d/b.txt", "b")]);
    sh.exec("shopt -s failglob; GLOBIGNORE='/d/*.txt'").unwrap();
    let r = sh.exec("echo /d/*.txt").unwrap();
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "rust-bash: no match: /d/*.txt\n");
    assert_eq!(r.exit_code, 1);
}

// ── Glob: extglob marking ───────────────────────────────────────────

#[test]
fn extglob_pattern_without_classic_metachars_is_glob_expanded() {
    // `@(f1|f2)` contains no `*`/`?`/`[`, so the word is only marked
    // glob-eligible by the extglob post-pass.
    let mut sh = shell_with_files(&[("/f1", ""), ("/f2", ""), ("/g1", "")]);
    let r = sh.exec("echo /@(f1|f2)").unwrap();
    assert_eq!(
        (r.stdout.as_str(), r.stderr.as_str(), r.exit_code),
        ("/f1 /f2\n", "", 0)
    );
}

// ── Command substitution corner cases ───────────────────────────────

#[test]
fn command_substitution_decodes_binary_stdout_lossily() {
    // gzip writes raw compressed bytes via stdout_bytes; the pipeline
    // boundary decodes them lossily, so $(...) preserves the visible text
    // portion instead of collapsing to the empty string.
    let mut sh = shell_with_files(&[("/f", "hello")]);
    let r = sh.exec("x=$(gzip -c /f); echo \"${#x}\"").unwrap();
    assert_eq!(r.stderr, "");
    assert_eq!(r.exit_code, 0);
    // The compressed stream is 25 bytes; lossy decoding maps the invalid
    // UTF-8 bytes to U+FFFD, yielding exactly 25 chars here.
    assert_eq!(r.stdout, "25\n");
}

#[test]
fn command_substitution_with_two_input_redirects_runs_normally() {
    // `$(< a < b)` is not the single-redirect `$(< file)` idiom, so it falls
    // through to executing a redirect-only command (no output).
    let mut sh = shell_with_files(&[("/a", "A"), ("/b", "B")]);
    let r = sh.exec("x=$(< /a < /b); echo \"[$x]\"").unwrap();
    assert_eq!(
        (r.stdout.as_str(), r.stderr.as_str(), r.exit_code),
        ("[]\n", "", 0)
    );
}

#[test]
fn command_substitution_with_output_redirect_only_is_not_file_read() {
    // `$(> file)` truncates the file and yields empty output; it must not be
    // mistaken for the `$(< file)` read idiom.
    let mut sh = shell_with_files(&[("/c", "content")]);
    let r = sh.exec("x=$(> /c); echo \"[$x]\"").unwrap();
    assert_eq!(
        (r.stdout.as_str(), r.stderr.as_str(), r.exit_code),
        ("[]\n", "", 0)
    );
    // Command substitution runs on a deep-cloned VFS, so the truncation does
    // not leak back to the parent shell's filesystem.
    assert_eq!(sh.read_file("/c").unwrap(), b"content");
}

// ── Substring error paths ───────────────────────────────────────────

#[test]
fn positional_slice_with_negative_length_is_an_error() {
    let (out, err, code) = run(r#"set -- a b; echo "before"; echo "${@:1:-1}""#);
    assert_eq!(out, "before\n");
    assert_eq!(err, "rust-bash: -1: substring expression < 0\n");
    assert_eq!(code, 1);
}

#[test]
fn array_slice_with_negative_offset_beyond_bounds_is_empty() {
    let (out, err, code) = run(r#"a=(x y); echo "[${a[@]: -5}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[]\n", "", 0));
}

#[test]
fn scalar_substring_with_offset_beyond_length_is_empty() {
    let (out, err, code) = run(r#"s=ab; echo "[${s:5}]""#);
    assert_eq!((out.as_str(), err.as_str(), code), ("[]\n", "", 0));
}
