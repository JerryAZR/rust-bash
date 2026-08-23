//! A sandboxed bash interpreter with a virtual filesystem.
//!
//! `rust-bash` executes bash scripts safely in-process — no containers, no VMs,
//! no host access. All file operations happen on a pluggable virtual filesystem
//! (in-memory by default), and configurable execution limits prevent runaway scripts.
//!
//! # Quick start
//!
//! ```rust
//! use rust_bash::RustBashBuilder;
//! use std::collections::HashMap;
//!
//! let mut shell = RustBashBuilder::new()
//!     .files(HashMap::from([
//!         ("/hello.txt".into(), b"hello world".to_vec()),
//!     ]))
//!     .build()
//!     .unwrap();
//!
//! let result = shell.exec("cat /hello.txt").unwrap();
//! assert_eq!(result.stdout, "hello world");
//! assert_eq!(result.exit_code, 0);
//! ```
//!
//! # Features
//!
//! - **80 built-in commands** — echo, cat, grep, awk, sed, jq, find, sort, diff, tar, and more
//! - **Full bash syntax** — pipelines, redirections, variables, control flow, functions,
//!   command substitution, globs, brace expansion, arithmetic, here-documents, case statements
//! - **Execution limits** — 10 configurable bounds (time, commands, loops, output size, etc.)
//! - **Unknown-command signaling** — `analyze_commands()` pre-flight and
//!   [`ExecResult::unresolved_commands`](crate::interpreter::ExecResult) report what the sandbox can't run
//! - **Multiple filesystem backends** — [`InMemoryFs`], [`OverlayFs`], [`MountableFs`]
//! - **Custom commands** — implement the [`VirtualCommand`] trait to add your own

pub mod api;
pub mod commands;
pub mod error;
pub mod interpreter;
pub mod platform;
mod shell_bytes;
pub mod vfs;

pub use api::{CommandAnalysis, RustBash, RustBashBuilder};
pub use commands::{CommandContext, CommandMeta, CommandResult, ExecCallback, VirtualCommand};
pub use error::{RustBashError, VfsError};
pub use interpreter::{
    ExecResult, ExecutionCounters, ExecutionLimits, InterpreterState, ShellOpts, Variable,
    VariableAttrs, VariableValue, builtin_names,
};
pub use vfs::{DirEntry, InMemoryFs, Metadata, MountableFs, NodeType, VirtualFs};

#[cfg(feature = "native-fs")]
pub use vfs::{OverlayDiff, OverlayFs, OverlayWrite};

#[cfg(test)]
mod parser_smoke_tests;
