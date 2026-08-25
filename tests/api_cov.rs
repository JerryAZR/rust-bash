//! Coverage tests for `src/api.rs`: builder options and `RustBash` API
//! methods not exercised elsewhere (VFS convenience methods, per-exec
//! overrides, positional params, command registration, input completeness).

use rust_bash::{
    CommandContext, CommandResult, NodeType, RustBash, RustBashBuilder, VirtualCommand,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

fn shell() -> RustBash {
    RustBashBuilder::new().build().unwrap()
}

// ── Builder ───────────────────────────────────────────────────────

#[test]
fn builder_default_trait_constructs_working_shell() {
    let mut sh = RustBashBuilder::default().build().unwrap();
    let r = sh.exec("echo hello").unwrap();
    assert_eq!(r.stdout, "hello\n");
    assert_eq!(r.exit_code, 0);
}

// ── Accessors and mutators ────────────────────────────────────────

#[test]
fn set_positional_params_exposes_dollar_n() {
    let mut sh = shell();
    sh.set_positional_params(vec!["alpha".into(), "beta".into()]);
    let r = sh.exec("echo $1-$2; echo $#").unwrap();
    assert_eq!(r.stdout, "alpha-beta\n2\n");
}

#[test]
fn fs_accessor_exposes_virtual_filesystem() {
    let sh = shell();
    // The builder seeds /bin command stubs.
    assert!(sh.fs().exists(Path::new("/bin/ls")));
    assert!(!sh.fs().exists(Path::new("/bin/definitely-not-a-command")));
}

// ── VFS convenience methods ───────────────────────────────────────

#[test]
fn write_file_creates_parent_dirs_and_read_file_roundtrips() {
    let sh = shell();
    sh.write_file("/deep/nested/dir/data.txt", b"payload")
        .unwrap();
    assert_eq!(
        sh.read_file("/deep/nested/dir/data.txt").unwrap(),
        b"payload"
    );
    assert!(sh.exists("/deep/nested/dir"));
}

#[test]
fn read_file_missing_path_is_not_found() {
    let sh = shell();
    let err = sh.read_file("/no/such/file").unwrap_err();
    assert_eq!(format!("{err}"), "No such file or directory: /no/such/file");
}

#[test]
fn mkdir_recursive_and_non_recursive() {
    let sh = shell();
    sh.mkdir("/a/b/c", true).unwrap();
    assert_eq!(sh.stat("/a/b/c").unwrap().node_type, NodeType::Directory);

    sh.mkdir("/single", false).unwrap();
    assert_eq!(sh.stat("/single").unwrap().node_type, NodeType::Directory);

    // Non-recursive mkdir with a missing parent fails.
    assert!(sh.mkdir("/missing-parent/child", false).is_err());
}

#[test]
fn readdir_lists_entries() {
    let sh = shell();
    sh.write_file("/listme/one.txt", b"1").unwrap();
    sh.write_file("/listme/two.txt", b"2").unwrap();
    let mut names: Vec<String> = sh
        .readdir("/listme")
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    assert_eq!(names, vec!["one.txt", "two.txt"]);
}

#[test]
fn stat_reports_file_metadata() {
    let sh = shell();
    sh.write_file("/meta.txt", b"abc").unwrap();
    let meta = sh.stat("/meta.txt").unwrap();
    assert_eq!(meta.node_type, NodeType::File);
    assert_eq!(meta.size, 3);
}

#[test]
fn remove_file_deletes_file() {
    let sh = shell();
    sh.write_file("/doomed.txt", b"x").unwrap();
    assert!(sh.exists("/doomed.txt"));
    sh.remove_file("/doomed.txt").unwrap();
    assert!(!sh.exists("/doomed.txt"));
    assert!(sh.remove_file("/doomed.txt").is_err());
}

#[test]
fn remove_dir_all_deletes_tree() {
    let sh = shell();
    sh.write_file("/tree/sub/leaf.txt", b"x").unwrap();
    sh.remove_dir_all("/tree").unwrap();
    assert!(!sh.exists("/tree"));
    assert!(!sh.exists("/tree/sub/leaf.txt"));
}

// ── Custom command registration ───────────────────────────────────

struct PingCommand;

impl VirtualCommand for PingCommand {
    fn name(&self) -> &str {
        "ping"
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        CommandResult {
            stdout: format!("pong:{}\n", args.join(",")),
            ..CommandResult::default()
        }
    }
}

#[test]
fn register_command_adds_executable_command() {
    let mut sh = shell();
    sh.register_command(Arc::new(PingCommand));
    assert!(sh.command_names().contains(&"ping"));
    let r = sh.exec("ping a b").unwrap();
    assert_eq!(r.stdout, "pong:a,b\n");
    assert_eq!(r.exit_code, 0);
}

// ── exec_with_overrides ───────────────────────────────────────────

#[test]
fn exec_with_overrides_env_applies_and_restores() {
    let mut sh = shell();
    let before = sh.exec("echo $USER").unwrap();
    assert_eq!(before.stdout, "user\n");

    let mut overrides = HashMap::new();
    overrides.insert("USER".to_string(), "alice".to_string()); // pre-existing var
    overrides.insert("BRAND_NEW".to_string(), "fresh".to_string()); // absent var

    let r = sh
        .exec_with_overrides("echo $USER $BRAND_NEW", Some(&overrides), None, None)
        .unwrap();
    assert_eq!(r.stdout, "alice fresh\n");

    // Both overrides are rolled back: USER returns to its old value and
    // BRAND_NEW is removed entirely.
    let after = sh.exec("echo $USER [$BRAND_NEW]").unwrap();
    assert_eq!(after.stdout, "user []\n");
}

#[test]
fn exec_with_overrides_cwd_applies_and_restores() {
    let mut sh = shell();
    let r = sh
        .exec_with_overrides("pwd", None, Some("/tmp"), None)
        .unwrap();
    assert_eq!(r.stdout, "/tmp\n");
    assert_eq!(sh.cwd(), "/");
}

#[test]
fn exec_with_overrides_stdin_feeds_heredoc() {
    let mut sh = shell();
    let r = sh
        .exec_with_overrides("cat", None, None, Some("line one\nline two"))
        .unwrap();
    assert_eq!(r.stdout, "line one\nline two\n");
}

#[test]
fn exec_with_overrides_stdin_containing_default_delimiter_uses_alternate() {
    let mut sh = shell();
    // The stdin payload contains the default heredoc delimiter, forcing the
    // implementation to fall back to its alternate delimiter.
    let r = sh
        .exec_with_overrides("cat", None, None, Some("has __EXEC_STDIN__ inside"))
        .unwrap();
    assert_eq!(r.stdout, "has __EXEC_STDIN__ inside\n");
}

// ── is_input_complete ─────────────────────────────────────────────

#[test]
fn is_input_complete_true_for_genuine_tokenize_error() {
    // `<<` immediately followed by a newline is a tokenize error that is NOT
    // an "incomplete input" error, so the input counts as complete.
    assert!(RustBash::is_input_complete("cat <<\nfoo"));
}
