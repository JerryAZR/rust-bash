//! Behavioral tests for assignment processing in the walker (array
//! initializers, append forms, readonly/limit errors, prefix assignments),
//! driven through the public `RustBash` API.

use rust_bash::{ExecutionLimits, RustBash, RustBashBuilder};

fn shell() -> RustBash {
    RustBashBuilder::new().build().unwrap()
}

fn run(script: &str) -> (String, String, i32) {
    let mut sh = shell();
    let r = sh.exec(script).unwrap();
    (r.stdout, r.stderr, r.exit_code)
}

// ── Array initializer edge cases ────────────────────────────────────

#[test]
fn array_item_with_bracket_prefix_but_no_equals_is_bare() {
    // `[0]x` is not a `[i]=v` item; it becomes a literal element.
    let (out, _, _) = run("a=([0]x); declare -p a");
    assert_eq!(out, "declare -a a=([0]=\"[0]x\")\n");
}

#[test]
fn array_item_with_unbalanced_bracket_after_brace_expansion() {
    // Brace expansion produces tokens with unbalanced `[`; both are kept
    // as literal elements.
    let (out, _, _) = run("a=(p[0{1,2}); declare -p a");
    assert_eq!(out, "declare -a a=([0]=\"p[01\" [1]=\"p[02\")\n");
}

#[test]
fn array_item_expanding_to_nothing_becomes_empty_element() {
    let (out, _, _) = run("e=; b=($e); declare -p b");
    assert_eq!(out, "declare -a b=([0]=\"\")\n");
}

#[test]
fn brace_expanded_items_expanding_to_nothing_become_empty_elements() {
    let (out, _, _) = run("e=; c=($e{,}); declare -p c");
    assert_eq!(out, "declare -a c=([0]=\"\" [1]=\"\")\n");
}

#[test]
fn assoc_item_expanding_to_nothing() {
    // Pinned actual behavior: the empty element pair leaves the assoc
    // array without printable entries.
    let (out, _, _) = run("declare -A m; e=; m=($e); declare -p m");
    assert_eq!(out, "declare -A m\n");
}

#[test]
fn declare_array_index_and_empty_value_rendering() {
    let (out, _, _) = run("declare -a a=([0]=x y); declare -p a");
    assert_eq!(out, "declare -a a=([0]=\"x\" [1]=\"y\")\n");
    let (out, _, _) = run("e=; declare -a a=([0]=$e); declare -p a");
    assert_eq!(out, "declare -a a=([0]=\"\")\n");
}

#[test]
fn array_item_with_unbalanced_bracket_single_token() {
    let (out, _, _) = run("a=([0x); declare -p a");
    assert_eq!(out, "declare -a a=([0]=\"[0x\")\n");
}

#[test]
fn declare_array_item_with_empty_middle_value() {
    // "$@" expands to multiple words including an empty one; the empty
    // word is rendered as '' in the reconstructed declare argument.
    let (out, _, _) = run("set -- a '' b; declare -a arr=([0]=\"$@\"); declare -p arr");
    assert_eq!(out, "declare -a arr=([0]=\"a\" [1]=\"\" [2]=\"b\")\n");
}

#[test]
fn declare_array_index_expansion_failure() {
    let (_, err, code) = run("set -u; declare -a a=([$u]=x); echo after");
    assert_eq!(err, "rust-bash: u: unbound variable\n");
    assert_eq!(code, 1);
}

// ── Readonly and limit errors ───────────────────────────────────────

#[test]
fn readonly_indexed_array_reassignment_errors() {
    let (_, err, code) = run("declare -ra a=(1); a=(2 3); echo after");
    assert_eq!(err, "rust-bash: line 1: a: readonly variable\n");
    assert_eq!(code, 1);
}

#[test]
fn readonly_assoc_array_reassignment_errors() {
    let (_, err, code) = run("declare -A m; readonly m; m=([j]=2); echo after");
    assert_eq!(err, "rust-bash: line 1: m: readonly variable\n");
    assert_eq!(code, 1);
    let (_, err, _) = run("declare -rA m=([k]=1); m=([j]=2); echo after");
    assert_eq!(err, "rust-bash: line 1: m: readonly variable\n");
}

#[test]
fn max_array_elements_limit_assoc_bare_assignment() {
    let limits = ExecutionLimits {
        max_array_elements: 2,
        ..ExecutionLimits::default()
    };
    let mut sh = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap();
    let r = sh.exec("declare -A m; m=([a]=1 [b]=2 [c]=3)");
    assert!(matches!(
        r,
        Err(rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_array_elements",
            limit_value: 2,
            actual_value: 3,
        })
    ));
}

#[test]
fn max_array_elements_limit_indexed() {
    let limits = ExecutionLimits {
        max_array_elements: 2,
        ..ExecutionLimits::default()
    };
    let mut sh = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap();
    let r = sh.exec("a=(1 2 3)");
    assert!(matches!(
        r,
        Err(rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_array_elements",
            limit_value: 2,
            actual_value: 3,
        })
    ));
}

#[test]
fn max_array_elements_limit_assoc() {
    let limits = ExecutionLimits {
        max_array_elements: 2,
        ..ExecutionLimits::default()
    };
    let mut sh = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap();
    let r = sh.exec("declare -A m=([a]=1 [b]=2 [c]=3)");
    assert!(matches!(
        r,
        Err(rust_bash::RustBashError::LimitExceeded {
            limit_name: "max_array_elements",
            limit_value: 2,
            actual_value: 3,
        })
    ));
}

// ── Array element append / negative indices ─────────────────────────

#[test]
fn append_to_scalar_element_zero() {
    let (out, _, _) = run("s=abc; s[0]+=x; echo $s");
    assert_eq!(out, "abcx\n");
}

#[test]
fn append_to_unset_scalar_element_converts_to_array() {
    // s[2]+=x on a scalar: current value at index 2 is empty, and the
    // variable becomes an indexed array. Pinned actual behavior.
    let (out, _, _) = run("s=abc; s[2]+=x; declare -p s");
    assert_eq!(out, "declare -a s=([2]=\"x\")\n");
}

#[test]
fn negative_index_on_scalar_writes_element_zero() {
    let (out, _, _) = run("s=5; s[-1]=9; echo $s");
    assert_eq!(out, "9\n");
}

#[test]
fn negative_index_on_unset_var_errors() {
    let (_, err, code) = run("u[-1]=3; echo after");
    assert_eq!(err, "rust-bash: line 1: u[-1]: bad array subscript\n");
    assert_eq!(code, 1);
}

#[test]
fn out_of_range_negative_index_errors() {
    let (_, err, _) = run("declare -a a=(1); a[-5]=3; echo after");
    assert_eq!(err, "rust-bash: line 1: a[-5]: bad array subscript\n");
}

// ── Bare assignment error paths ─────────────────────────────────────

#[test]
fn bare_assign_array_to_element_is_nonfatal() {
    let (out, err, _) = run("a[0]=(x); echo after");
    assert_eq!(out, "after\n");
    assert_eq!(err, "rust-bash: a: cannot assign array to array element\n");
}

#[test]
fn bare_readonly_assignment_aborts_command_list() {
    let (out, err, code) = run("readonly r=1; r=2; echo after");
    assert_eq!(out, "");
    assert_eq!(err, "rust-bash: line 1: r: readonly variable\n");
    assert_eq!(code, 1);
}

#[test]
fn bare_readonly_assignment_aborts_posix() {
    let (out, _, code) = run("set -o posix; readonly r=1; r=2; echo after");
    assert_eq!(out, "");
    assert_eq!(code, 1);
}

#[test]
fn bare_assignment_with_redirect_truncates_file() {
    let (out, _, _) = run("a=1 > /f; cat /f");
    assert_eq!(out, "");
}

#[test]
fn bare_assignment_cmdsub_stderr_propagates() {
    let (out, err, _) = run("a=$(echo e >&2; echo o); echo $a");
    assert_eq!(out, "o\n");
    assert_eq!(err, "e\n");
}

#[test]
fn bare_assignment_nounset_arithmetic_is_fatal() {
    let (_, err, code) = run("set -u; x=$(( y + 1 )); echo after");
    assert_eq!(err, "rust-bash: line 1: y: unbound variable\n");
    assert_eq!(code, 1);
}

#[test]
fn bare_assignment_nounset_expansion_error() {
    let (_, err, code) = run("set -u; x=$y; echo after");
    assert_eq!(err, "rust-bash: y: unbound variable\n");
    assert_eq!(code, 1);
}

// ── Prefix (temp-binding) assignment error paths ────────────────────

#[test]
fn prefix_assignment_process_error_still_runs_command() {
    let (out, err, code) = run("a[0]=(x) echo hi");
    assert_eq!(out, "hi\n");
    assert_eq!(
        err,
        "rust-bash: execution error: a: cannot assign array to array element\n"
    );
    assert_eq!(code, 0);
}

#[test]
fn prefix_readonly_assignment_still_runs_command() {
    let (out, err, _) = run("readonly r=1; r=2 echo hi; echo after");
    assert_eq!(out, "hi\nafter\n");
    assert_eq!(err, "rust-bash: line 1: r: readonly variable\n");
}

#[test]
fn prefix_assignment_nounset_arithmetic_still_runs_command() {
    let (out, err, _) = run("set -u; x=$(( y + 1 )) echo hi; echo after");
    assert_eq!(out, "hi\nafter\n");
    assert_eq!(err, "rust-bash: execution error: y: unbound variable\n");
}

// ── Empty command name (`$(false)`) with assignments ────────────────

#[test]
fn empty_command_assignment_process_error() {
    let (out, err, _) = run("a[0]=(x) $(false); echo after");
    assert_eq!(out, "after\n");
    assert_eq!(err, "rust-bash: a: cannot assign array to array element\n");
}

#[test]
fn empty_command_assignment_nounset_arithmetic_is_fatal() {
    let (_, err, code) = run("set -u; x=$(( y + 1 )) $(false); echo after");
    assert_eq!(err, "rust-bash: line 1: y: unbound variable\n");
    assert_eq!(code, 1);
}

#[test]
fn empty_command_readonly_assignment_still_continues() {
    let (out, err, _) = run("readonly r=1; r=2 $(false); echo after");
    assert_eq!(out, "after\n");
    assert_eq!(err, "rust-bash: line 1: r: readonly variable\n");
}

#[test]
fn empty_command_assignment_with_redirect() {
    let (out, _, _) = run("a=1 $(false) > /f; cat /f");
    assert_eq!(out, "");
}

#[test]
fn empty_command_assignment_cmdsub_stderr() {
    let (out, err, _) = run("a=$(echo e >&2) $(false); echo rc=$?");
    assert_eq!(out, "rc=0\n");
    assert_eq!(err, "e\n");
}

// ── stderr→stdout merge with persistent fd 2 ────────────────────────

#[test]
fn merge_stderr_to_stdout_restores_persistent_fd2() {
    // With fd2 persistently redirected to /err, `2>&1` temporarily merges
    // stderr into stdout for one command; afterwards fd2 points at /err
    // again, so the second error lands in the file.
    let (out, err, _) = run(
        "exec 2> /err; cat /nope1 2>&1; cat /nope2 2>/dev/null; cat /nope3; \
         exec 2> /dev/stderr; cat /err",
    );
    assert_eq!(
        out,
        "cat: /nope1: No such file or directory: /nope1\n\
         cat: /nope3: No such file or directory: /nope3\n"
    );
    assert_eq!(err, "");
}

#[test]
fn emptied_command_assignment_limit_exceeded_propagates_as_error() {
    use rust_bash::{ExecutionLimits, RustBashBuilder};
    let limits = ExecutionLimits {
        max_string_length: 10,
        ..ExecutionLimits::default()
    };
    let mut sh = RustBashBuilder::new()
        .execution_limits(limits)
        .build()
        .unwrap();
    // `$(false)` expands the command name to empty, so the assignment is
    // processed on the empty-command path (walker step 4b).
    let err = sh.exec("x=aaaaaaaaaaaaaaaaaaaa $(false)").unwrap_err();
    assert!(
        matches!(err, rust_bash::RustBashError::LimitExceeded { .. }),
        "unexpected error: {err:?}"
    );
}
