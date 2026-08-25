//! Coverage tests for `src/commands/jq_cmd.rs`: argument-parsing error
//! paths, compile/load error formatting, file-input errors, and numeric
//! output conversion edge cases.

use rust_bash::RustBash;

fn shell() -> RustBash {
    rust_bash::RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> (String, String, i32) {
    let mut sh = shell();
    let r = sh.exec(script).unwrap();
    (r.stdout, r.stderr, r.exit_code)
}

// ── Option argument errors ────────────────────────────────────────

#[test]
fn arg_missing_value_is_an_error() {
    let (stdout, stderr, code) = run("jq --arg name");
    assert_eq!(code, 2);
    assert_eq!(stdout, "");
    assert_eq!(stderr, "jq: --arg requires NAME VALUE\n");
}

#[test]
fn argjson_missing_value_is_an_error() {
    let (stdout, stderr, code) = run("jq --argjson name");
    assert_eq!(code, 2);
    assert_eq!(stdout, "");
    assert_eq!(stderr, "jq: --argjson requires NAME VALUE\n");
}

#[test]
fn unknown_combined_short_option_is_an_error() {
    let (stdout, stderr, code) = run("jq -Z '.' <<< 'null'");
    assert_eq!(code, 2);
    assert_eq!(stdout, "");
    assert_eq!(stderr, "jq: Unknown option: -Z\n");
}

#[test]
fn unknown_long_option_is_an_error() {
    let (stdout, stderr, code) = run("jq --bogus '.' <<< 'null'");
    assert_eq!(code, 2);
    assert_eq!(stdout, "");
    assert_eq!(stderr, "jq: Unknown option: --bogus\n");
}

// ── Combined short flags ──────────────────────────────────────────

#[test]
fn combined_sort_keys_and_join_output() {
    let (stdout, stderr, code) = run("echo '{\"b\":1,\"a\":2}' | jq -Sj '.'");
    assert_eq!(code, 0);
    assert_eq!(stderr, "");
    // Sorted keys, pretty-printed, no trailing newline (-j).
    assert_eq!(stdout, "{\n  \"a\": 2,\n  \"b\": 1\n}");
}

#[test]
fn combined_exit_status_and_null_input() {
    let (stdout, _, code) = run("jq -en 'false'");
    assert_eq!(code, 1);
    assert_eq!(stdout, "false\n");
}

// ── Compile and load errors ───────────────────────────────────────

#[test]
fn undefined_function_is_a_compile_error() {
    let (stdout, stderr, code) = run("jq 'nosuchfunc' <<< '{}'");
    assert_eq!(code, 3);
    assert_eq!(stdout, "");
    assert_eq!(
        stderr,
        "jq: compile error: nosuchfunc: undefined Filter(0) 'nosuchfunc'\n"
    );
}

#[test]
fn import_reports_module_loading_not_supported() {
    let (stdout, stderr, code) = run(r#"jq 'import "foo" as f; 1' <<< 'null'"#);
    assert_eq!(code, 3);
    assert_eq!(stdout, "");
    assert_eq!(
        stderr,
        "jq: compile error: import \"foo\" as f; 1: foo: module loading not supported\n"
    );
}

#[test]
fn lex_error_is_reported_with_count() {
    let (stdout, stderr, code) = run("jq '~' <<< 'null'");
    assert_eq!(code, 3);
    assert_eq!(stdout, "");
    assert_eq!(stderr, "jq: compile error: ~: 1 lex error(s)\n");
}

// ── File input errors ─────────────────────────────────────────────

#[test]
fn missing_input_file_is_an_error() {
    let (stdout, stderr, code) = run("jq '.' /nonexistent.json");
    assert_eq!(code, 2);
    assert_eq!(stdout, "");
    assert_eq!(
        stderr,
        "jq: /nonexistent.json: No such file or directory: /nonexistent.json\n"
    );
}

// ── Numeric output conversion ─────────────────────────────────────

#[test]
fn float_output_preserves_fraction() {
    let (stdout, stderr, code) = run("jq -n '0.5'");
    assert_eq!(code, 0);
    assert_eq!(stderr, "");
    assert_eq!(stdout, "0.5\n");
}

#[test]
fn non_finite_numbers_render_as_null() {
    // jaq renders Infinity/NaN; serde_json cannot represent them, so the
    // formatter falls back to null.
    // Suspected divergence: real jq prints 1.7976931348623157e+308 for
    // `infinite` (nan -> null matches real jq).
    let (stdout, _, code) = run("jq -n 'infinite'");
    assert_eq!(code, 0);
    assert_eq!(stdout, "null\n");

    let (stdout, _, code) = run("jq -n 'nan'");
    assert_eq!(code, 0);
    assert_eq!(stdout, "null\n");
}

#[test]
fn non_string_object_keys_are_stringified() {
    let (stdout, stderr, code) = run("jq -n '{(1): 2}'");
    assert_eq!(code, 0);
    assert_eq!(stderr, "");
    assert_eq!(stdout, "{\n  \"1\": 2\n}\n");
}
