//! Coverage tests for `src/api.rs`: builder options and `RustBash` API
//! methods not exercised elsewhere (VFS convenience methods, per-exec
//! overrides, positional params, command registration, input completeness).

use rust_bash::{
    CommandContext, CommandResult, NodeType, RustBash, RustBashBuilder, VirtualCommand,
    env_from_host,
};
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
fn set_env_sets_exported_var_visible_to_exec() {
    let mut sh = shell();
    sh.set_env("FOO", "bar");
    assert_eq!(sh.exec("echo $FOO").unwrap().stdout, "bar\n");
    // Exported: commands see it in their environment, `export -p` lists it.
    assert!(sh.exec("env").unwrap().stdout.contains("FOO=bar"));
    assert!(
        sh.exec("export -p")
            .unwrap()
            .stdout
            .contains("declare -x FOO=\"bar\""),
        "export -p should list FOO as exported"
    );
}

#[test]
fn set_env_overwrites_and_unset_env_removes() {
    let mut sh = shell();
    sh.set_env("FOO", "one");
    sh.set_env("FOO", "two");
    assert_eq!(sh.exec("echo $FOO").unwrap().stdout, "two\n");
    sh.unset_env("FOO");
    assert_eq!(sh.exec("echo [$FOO]").unwrap().stdout, "[]\n");
}

#[test]
fn env_from_host_reports_found_and_missing() {
    // PATH is effectively always set on host machines (Linux, macOS, Windows).
    let (found, missing) = env_from_host(&["PATH", "RUST_BASH_DEFINITELY_UNSET_XYZZY"]);
    assert!(found.contains_key("PATH"));
    assert!(!found.contains_key("RUST_BASH_DEFINITELY_UNSET_XYZZY"));
    assert_eq!(missing, ["RUST_BASH_DEFINITELY_UNSET_XYZZY"]);
}

#[test]
fn env_from_host_reports_invalid_names_instead_of_panicking() {
    // `std::env::var` would panic on these; they must be reported as missing.
    let (found, missing) = env_from_host(&["", "A=B"]);
    assert!(found.is_empty());
    assert_eq!(missing, ["", "A=B"]);
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

// ── set_stdin ───────────────────────────────────────────────────────

#[test]
fn set_stdin_feeds_first_command_exactly() {
    let mut sh = shell();
    sh.set_stdin(Some("line one\nline two".to_string()));
    let r = sh.exec("cat").unwrap();
    assert_eq!(r.stdout, "line one\nline two");
}

#[test]
fn set_stdin_is_byte_exact_no_added_newline() {
    // The old heredoc-splice approach silently appended a newline; the
    // stdin channel must deliver the payload unchanged.
    let mut sh = shell();
    sh.set_stdin(Some("abc".to_string()));
    let r = sh.exec("cat | wc -c").unwrap();
    assert_eq!(r.stdout.trim(), "3");
}

#[test]
fn set_stdin_payloads_are_never_parsed() {
    // Payloads containing the old sentinel strings (or anything else)
    // pass through untouched — the channel does no text substitution.
    let mut sh = shell();
    sh.set_stdin(Some(
        "a\n__EXEC_STDIN__\nb\n__EXEC_STDIN_BOUNDARY__\nc".to_string(),
    ));
    let r = sh.exec("cat").unwrap();
    assert_eq!(r.stdout, "a\n__EXEC_STDIN__\nb\n__EXEC_STDIN_BOUNDARY__\nc");
    assert_eq!(r.exit_code, 0);
}

#[test]
fn set_stdin_with_comment_tailed_script() {
    // The script text is never spliced, so a trailing comment cannot eat
    // the stdin channel.
    let mut sh = shell();
    sh.set_stdin(Some("payload".to_string()));
    let r = sh.exec("cat # trailing note").unwrap();
    assert_eq!(r.stdout, "payload");
}

#[test]
fn set_stdin_is_one_shot() {
    // Consumed by the next exec; a stale override must not leak forward.
    let mut sh = shell();
    sh.set_stdin(Some("first".to_string()));
    let r = sh.exec("cat").unwrap();
    assert_eq!(r.stdout, "first");
    let r = sh.exec("cat; echo status=$?").unwrap();
    assert!(r.stdout.contains("status=0"), "{:?}", r.stdout);
}

#[test]
fn set_stdin_none_clears_pending() {
    let mut sh = shell();
    sh.set_stdin(Some("ignored".to_string()));
    sh.set_stdin(None);
    let r = sh.exec("cat; echo status=$?").unwrap();
    assert!(r.stdout.contains("status=0"));
}

// ── is_input_complete ─────────────────────────────────────────────

#[test]
fn is_input_complete_true_for_genuine_tokenize_error() {
    // `<<` immediately followed by a newline is a tokenize error that is NOT
    // an "incomplete input" error, so the input counts as complete.
    assert!(RustBash::is_input_complete("cat <<\nfoo"));
}
