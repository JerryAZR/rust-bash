# rust-bash Recipes

Task-oriented guides for common use cases. Each recipe is a self-contained document showing how to accomplish a specific task with rust-bash.

## Recipes

| Recipe | Description |
|--------|-------------|
| [Getting Started](getting-started.md) | Embed rust-bash in a Rust application, execute scripts, inspect results |
| [Custom Commands](custom-commands.md) | Implement and register domain-specific commands via the `VirtualCommand` trait |
| [Filesystem Backends](filesystem-backends.md) | Choose between InMemoryFs, OverlayFs, and MountableFs |
| [Execution Limits](execution-limits.md) | Configure resource bounds for different trust levels |
| [Multi-Step Sessions](multi-step-sessions.md) | Maintain state across multiple `exec()` calls for agents |
| [Text Processing Pipelines](text-processing.md) | Build data pipelines with grep, sed, awk, jq, sort, and more |
| [Embedding in an AI Agent](ai-agent-tool.md) | Set up rust-bash as a sandboxed tool for LLM function calling |
| [Agent Sandbox Integration](agent-sandbox-integration.md) | Best-effort sandbox loop: unknown-command native fallback + OverlayFs write tracking/apply |
| [Error Handling](error-handling.md) | Handle errors, use `set -e`/`set -u`/`set -o pipefail`, and recover gracefully |
| [Shell Scripting Features](shell-scripting.md) | Variables, control flow, functions, arithmetic, subshells, and more |

## Contributing a Recipe

Recipes should be:
1. **Task-focused** — "how to do X", not "what is X"
2. **Self-contained** — include all code needed to follow along
3. **Tested** — all code examples should actually work
4. **Concise** — get to the point quickly, link to the guidebook for deep dives
