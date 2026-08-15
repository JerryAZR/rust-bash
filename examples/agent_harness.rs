//! Agent harness integration example: sandbox-first execution with native
//! fallback and write tracking.
//!
//! Demonstrates the workflow documented in
//! `docs/recipes/agent-sandbox-integration.md`:
//!
//! 1. `analyze_commands()` pre-flight — if the script uses commands the
//!    sandbox doesn't implement, report them and (in a real harness) rerun
//!    the script natively on the host.
//! 2. `exec()` with `unresolved_commands` reporting as the runtime backstop
//!    for dynamically-built command names.
//! 3. `OverlayFs::diff()` — inspect exactly which files the sandboxed run
//!    created/modified/deleted, then apply writes inside the project root
//!    and print the ones outside it for a prompt decision.
//!
//! Run with:
//!
//! ```text
//! cargo run --example agent_harness -- path/to/project
//! ```
//!
//! Nothing on disk is modified unless you pass `--apply`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rust_bash::{NodeType, OverlayFs, OverlayWrite, RustBash, RustBashBuilder, RustBashError};

struct Harness {
    shell: RustBash,
    overlay: Arc<OverlayFs>,
    project_root: PathBuf,
    apply: bool,
}

impl Harness {
    fn new(project_root: PathBuf, apply: bool) -> Result<Self, RustBashError> {
        let overlay = Arc::new(
            OverlayFs::new(&project_root)
                .map_err(|e| RustBashError::Execution(format!("overlay root: {e}")))?,
        );
        let shell = RustBashBuilder::new()
            .fs(overlay.clone())
            .cwd("/")
            .abort_on_unresolved_commands(true)
            .build()?;
        Ok(Self {
            shell,
            overlay,
            project_root,
            apply,
        })
    }

    /// Run a script: sandboxed when possible, native fallback otherwise.
    /// Returns true when the script ran in the sandbox.
    fn run(&mut self, script: &str) -> Result<bool, RustBashError> {
        // 1. Pre-flight: statically visible unknown commands?
        let analysis = self.shell.analyze_commands(script)?;
        if !analysis.unresolved.is_empty() {
            println!("pre-flight: unresolved commands: {:?}", analysis.unresolved);
            self.run_natively(script);
            return Ok(false);
        }

        // 2. Execute in the sandbox; abort at the first runtime miss.
        let result = self.shell.exec(script)?;
        println!(
            "sandbox: exit={} stdout={} bytes, stderr={} bytes",
            result.exit_code,
            result.stdout.len(),
            result.stderr.len()
        );
        if !result.unresolved_commands.is_empty() {
            // Dynamically-built names that pre-flight could not see.
            println!(
                "runtime: unresolved commands: {:?}",
                result.unresolved_commands
            );
            self.run_natively(script);
            return Ok(false);
        }

        // 3. Inspect and (optionally) apply the write set. Individual
        // failures need no bookkeeping: sync() below drops only the entries
        // that now match disk, so anything that failed to apply (or was
        // skipped for a prompt) stays visible as a pending change.
        let diff = self.overlay.diff();
        for w in &diff.writes {
            match self.classify(&w.path) {
                Apply::Auto => {
                    let host = self.host_path(&w.path);
                    self.apply_write(&host, w);
                }
                Apply::Prompt => {
                    let host = self.host_path(&w.path);
                    self.report_for_prompt(&host, w);
                }
                Apply::Discard => {}
            }
        }
        for p in &diff.deletions {
            match self.classify(p) {
                Apply::Auto => self.apply_delete(&self.host_path(p)),
                Apply::Prompt => println!("PROMPT delete: {}", self.host_path(p).display()),
                Apply::Discard => {}
            }
        }

        // 4. Reconcile: applied writes/deletions drop out of the overlay;
        // whatever still differs from disk is the actionable remainder.
        self.overlay.sync();
        let remaining = self.overlay.diff();
        // Sandbox-internal entries (classify == Discard) are intentionally
        // never applied and stay in the overlay inertly — only count what
        // the harness still owes a decision or a retry on.
        let owed: usize = remaining
            .writes
            .iter()
            .filter(|w| !matches!(self.classify(&w.path), Apply::Discard))
            .count()
            + remaining
                .deletions
                .iter()
                .filter(|p| !matches!(self.classify(p), Apply::Discard))
                .count();
        if owed > 0 {
            println!("pending after sync: {owed} entries need a retry or decision");
        }
        Ok(true)
    }

    /// Harness policy: a real harness runs the script with a host shell
    /// (bash on Unix, Git Bash on Windows). Illustrated with a message.
    fn run_natively(&self, script: &str) {
        println!("native: would rerun via host shell: {script:?}");
    }

    fn host_path(&self, vfs: &Path) -> PathBuf {
        let s = vfs.to_string_lossy();
        let rel = s.trim_start_matches('/');
        self.project_root.join(rel)
    }

    /// Harness policy:
    /// - sandbox-internal writes (default `/bin`, `/dev`, `/home`, `/tmp`,
    ///   `/usr` layout) are discarded;
    /// - writes into the project's `.git` directory require a prompt;
    /// - everything else (i.e. the project subtree) is auto-applied.
    ///
    /// A harness with a wider overlay root would additionally prompt for
    /// any path outside the project.
    fn classify(&self, vfs: &Path) -> Apply {
        let v = vfs.to_string_lossy();
        for internal in ["/bin", "/dev", "/home", "/tmp", "/usr"] {
            if v == internal || v.starts_with(&format!("{internal}/")) {
                return Apply::Discard;
            }
        }
        if v == "/.git" || v.starts_with("/.git/") {
            return Apply::Prompt;
        }
        Apply::Auto
    }

    fn apply_write(&self, host: &Path, w: &OverlayWrite) {
        if !self.apply {
            println!(
                "would write: {} ({:?}, {} bytes)",
                host.display(),
                w.node_type,
                w.content.len()
            );
            return;
        }
        match w.node_type {
            NodeType::Directory => {
                let _ = std::fs::create_dir_all(host);
            }
            NodeType::File => {
                if let Some(parent) = host.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(host, &w.content) {
                    eprintln!("apply failed for {}: {e}", host.display());
                }
            }
            NodeType::Symlink => {
                // Symlink creation needs platform APIs and privileges
                // (Developer Mode on Windows); left as harness policy.
                let target = String::from_utf8_lossy(&w.content);
                println!("symlink (not applied): {} -> {target}", host.display());
            }
        }
    }

    fn apply_delete(&self, host: &Path) {
        if !self.apply {
            println!("would delete: {}", host.display());
            return;
        }
        if host.is_dir() {
            let _ = std::fs::remove_dir_all(host);
        } else {
            let _ = std::fs::remove_file(host);
        }
    }

    fn report_for_prompt(&self, host: &Path, w: &OverlayWrite) {
        println!(
            "PROMPT write: {} ({:?}, {} bytes)",
            host.display(),
            w.node_type,
            w.content.len()
        );
    }
}

enum Apply {
    Auto,
    Prompt,
    Discard,
}

fn main() -> Result<(), RustBashError> {
    let mut args = std::env::args().skip(1);
    let project_root = PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let apply = args.next().as_deref() == Some("--apply");

    let mut harness = Harness::new(project_root, apply)?;

    // A script the sandbox can fully handle: reads project files, writes
    // stay in the overlay until applied.
    let sandboxed =
        "ls; grep -c fn src/main.rs 2>/dev/null || echo 'no src/'; echo demo > sandbox-demo.txt";
    println!("=== sandboxed script ===");
    assert!(harness.run(sandboxed)?);

    // A script needing a native tool: reported, never executed here.
    let native = "grep -r TODO . | head -3; git status";
    println!("\n=== native-fallback script ===");
    assert!(!harness.run(native)?);

    // Dynamically-built command name: invisible to static analysis, caught
    // at execution time by `unresolved_commands` (with abort-on-miss).
    let dynamic = "cmd=git; $cmd log --oneline -3";
    println!("\n=== runtime-backstop script ===");
    assert!(!harness.run(dynamic)?);

    Ok(())
}
