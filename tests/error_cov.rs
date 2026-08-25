//! Coverage tests for `src/error.rs`: `Display`, `Error::source`, and
//! `From<VfsError>` for every `RustBashError` variant.

use rust_bash::{RustBashError, VfsError};
use std::error::Error;

#[test]
fn rustbash_error_display_all_variants() {
    let cases: Vec<(RustBashError, &str)> = vec![
        (
            RustBashError::Parse("bad syntax".into()),
            "parse error: bad syntax",
        ),
        (
            RustBashError::Execution("boom".into()),
            "execution error: boom",
        ),
        (
            RustBashError::ExpansionError {
                message: "var: unset".into(),
                exit_code: 1,
                should_exit: false,
            },
            "expansion error: var: unset",
        ),
        (
            RustBashError::FailGlob {
                pattern: "*.rs".into(),
            },
            "no match: *.rs",
        ),
        (
            RustBashError::RedirectFailed("ambiguous redirect".into()),
            "rust-bash: ambiguous redirect",
        ),
        (
            RustBashError::LimitExceeded {
                limit_name: "max_commands",
                limit_value: 10,
                actual_value: 11,
            },
            "limit exceeded: max_commands (11) exceeded limit (10)",
        ),
        (
            RustBashError::Vfs(VfsError::NotFound("/x".into())),
            "vfs error: No such file or directory: /x",
        ),
        (RustBashError::Timeout, "execution timed out"),
    ];

    for (err, expected) in cases {
        assert_eq!(format!("{err}"), expected);
    }
}

#[test]
fn rustbash_error_source_only_for_vfs_variant() {
    let vfs_err = RustBashError::Vfs(VfsError::PermissionDenied("/secret".into()));
    let source = vfs_err.source().expect("Vfs variant must have a source");
    let vfs_source = source
        .downcast_ref::<VfsError>()
        .expect("source must be the wrapped VfsError");
    assert_eq!(*vfs_source, VfsError::PermissionDenied("/secret".into()));

    // Every other variant has no source.
    let no_source_cases = [
        RustBashError::Parse("p".into()),
        RustBashError::Execution("e".into()),
        RustBashError::ExpansionError {
            message: "m".into(),
            exit_code: 1,
            should_exit: true,
        },
        RustBashError::FailGlob {
            pattern: "g".into(),
        },
        RustBashError::RedirectFailed("r".into()),
        RustBashError::LimitExceeded {
            limit_name: "l",
            limit_value: 1,
            actual_value: 2,
        },
        RustBashError::Timeout,
    ];
    for err in no_source_cases {
        assert!(err.source().is_none(), "unexpected source for {err}");
    }
}

#[test]
fn rustbash_error_from_vfs_error_wraps_variant() {
    let err: RustBashError = VfsError::NotADirectory("/file".into()).into();
    assert!(matches!(
        err,
        RustBashError::Vfs(VfsError::NotADirectory(_))
    ));
    assert_eq!(format!("{err}"), "vfs error: Not a directory: /file");
}
