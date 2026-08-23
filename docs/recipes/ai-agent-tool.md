# Embedding in an AI Agent

## Goal

Use rust-bash as a bash execution tool for LLM-powered agents. The shell provides a sandboxed environment where the AI can run commands, inspect files, and process data — without containers, VMs, or host filesystem access.

## Why rust-bash for AI Agents?

| Feature | rust-bash | Docker/VM | Host bash |
|---------|-----------|-----------|-----------|
| Startup time | Microseconds | Seconds | Microseconds |
| Isolation | Virtual FS, execution limits | Full OS-level | None |
| Memory footprint | KBs | MBs–GBs | N/A |
| Custom commands | VirtualCommand trait | Mount scripts | PATH |
| Reproducible FS | Yes (InMemoryFs) | Mostly | No |

## Basic Agent Setup

```rust
use rust_bash::{RustBashBuilder, RustBashError, ExecutionLimits};
use std::collections::HashMap;
use std::time::Duration;

struct AgentShell {
    shell: rust_bash::RustBash,
}

impl AgentShell {
    fn new() -> Self {
        let shell = RustBashBuilder::new()
            .env(HashMap::from([
                ("HOME".into(), "/home/agent".into()),
                ("USER".into(), "agent".into()),
            ]))
            .cwd("/home/agent")
            .execution_limits(ExecutionLimits {
                max_command_count: 5_000,
                max_execution_time: Duration::from_secs(10),
                max_output_size: 512 * 1024, // 512 KB
                ..Default::default()
            })
            .build()
            .unwrap();

        Self { shell }
    }

    /// Execute a command and return a structured result for the LLM.
    fn run(&mut self, command: &str) -> AgentResult {
        match self.shell.exec(command) {
            Ok(result) => AgentResult {
                success: result.exit_code == 0,
                stdout: truncate(&result.stdout, 4096),
                stderr: truncate(&result.stderr, 1024),
                exit_code: result.exit_code,
                error: None,
            },
            Err(RustBashError::LimitExceeded { limit_name, .. }) => AgentResult {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: -1,
                error: Some(format!("Resource limit exceeded: {limit_name}")),
            },
            Err(e) => AgentResult {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: -1,
                error: Some(format!("{e}")),
            },
        }
    }
}

struct AgentResult {
    success: bool,
    stdout: String,
    stderr: String,
    exit_code: i32,
    error: Option<String>,
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let end = s.char_indices()
            .take_while(|(i, _)| *i < max)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}... [truncated, {} total bytes]", &s[..end], s.len())
    }
}
```

## Tool Definition for Function Calling

```json
{
  "name": "bash",
  "description": "Execute a bash command in a sandboxed environment. The environment has a virtual filesystem, 80+ Unix commands (grep, sed, awk, jq, find, etc.), and full bash syntax (variables, loops, functions, pipes, redirections). State persists between calls.",
  "parameters": {
    "type": "object",
    "properties": {
      "command": {
        "type": "string",
        "description": "The bash command to execute"
      }
    },
    "required": ["command"]
  }
}
```

## Guardrails Against Runaway Scripts

The combination of execution limits and the virtual filesystem keeps careless
scripts from doing real damage or hanging the agent loop:

1. **No unintended disk writes** — `InMemoryFs` by default, `OverlayFs` for
   reviewable writes over a real directory
2. **Resource bounds** — time, commands, output size all capped
3. **No process spawning** — all commands run in-process; no `std::process::Command`
4. **Structured errors** — `LimitExceeded` reports exactly which limit was hit

These are guardrails against mistakes, not a security boundary — see the
[agent sandbox pattern](agent-sandbox-integration.md) for what is and isn't
promised. See [Execution Limits](execution-limits.md) for detailed configuration.
