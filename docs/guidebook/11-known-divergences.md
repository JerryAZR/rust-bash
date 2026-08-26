# Chapter 11: Known Divergences

This chapter is the consolidated registry of places where rust-bash's actual behavior diverges from real bash, GNU coreutils, gawk, or POSIX. Every entry is **pinned by a test** (mostly written during the crate-wide coverage campaign) that asserts the *actual* behavior with a `DIVERGENCE` / `pinned` comment.

## Policy

- Divergences are **pinned, never silently fixed**. A behavior change to any entry below requires updating both the pinning test and this registry in the same commit.
- Entries marked *(suspected)* were reasoned from documentation, not verified against a live reference implementation.
- Systematic patterns (same divergence across many commands) are listed once with their command list.
- The registry is descriptive, not a commitment to fix. Entries are candidates for fidelity work, prioritized by how likely an agent-generated script is to trip over them.

## 1. Functional bugs (best fix candidates)

| Behavior | Expected | Pinned in |
|---|---|---|
| `[[ abc == @(a\|b)c ]]` with `shopt -s extglob` does **not** match | bash matches (exit 0) | `tests/walker_extended_test.rs::extglob_divergence` |
| `nocasematch` + extglob: `[[ ABC == @(a\|b)c ]]` does not match | bash matches | `tests/walker_extended_test.rs::nocasematch_extglob_divergence` |
| Nested ternary in parens `$(( (1 ? 2 : 0 ? 3 : 4) + 0 ))` → `expected RParen` | bash prints `2` (`skip_ternary_branch` consumes the `)`) | `tests/arithmetic_eval.rs::nested_ternary_inside_parens_divergence` |
| `expand -t 0,` → **division-by-zero panic** in `next_tab_stop` | GNU: tab stop 0 falls back / errors cleanly | noted in `src/commands/text.rs` (latent; no test — a panic aborts the host) |
| `printf`/`awk` `format_scientific(inf)` → `"NaNe+2147483647"` | `inf` | noted in `src/commands/text.rs` (latent) |
| `A=1 [[ x = y ]]` runs a command literally named `[[` | bash parses an extended test with a temp env binding | `tests/interp_core_cov.rs` |
| `echo ${ ;}` prints `${;}` | bash: bad substitution | `tests/interp_core_cov.rs` |
| `${ case $x in (a) …; }` ksh-style command substitution accepted | bash: bad substitution (intentional brush-parser feature) | `tests/interp_core_cov.rs` |

## 2. Interpreter & expansion

| Behavior | Expected | Pinned in |
|---|---|---|
| `${v%${y:=c}}` expands the default in pattern context but never assigns `y` | bash assigns | `tests/expansion_parameter.rs` |
| `${v%$(echo b)}` drops the command substitution (empty pattern, no strip) | bash executes it | `tests/expansion_parameter.rs` |
| `${v%${a[@]:-y}}` with empty `a` yields empty pattern (mutable path yields `y`) | consistent | `tests/expansion_parameter.rs` |
| `${x:?}` prints an empty message | bash: "parameter null or not set" | `tests/expansion_parameter.rs` |
| `${1:=x}` silently drops the assignment | bash: "cannot assign to positional parameter" | `tests/expansion_parameter.rs` |
| Negative array subscripts accepted; out-of-range only warns with exit 0 | bash rejects entirely | `tests/expansion_parameter.rs` |
| `arr[-5]` via nameref clamps to index 0 | bash: bad array subscript | `tests/interp_core_cov.rs` |
| `declare -n x=t; declare -A x=([k]=v)` errors with an empty variable name | bash rejects nameref+array upfront | `tests/interp_core_cov.rs` |
| `~N` / `~-N` collapse to bare `~` (no dir-stack resolution) | bash resolves via dir stack | `tests/expansion_transforms.rs` |
| `@P` prompt date/time escapes are fixed strings | bash uses wall clock (deliberate sandbox determinism) | `tests/expansion_transforms.rs` |
| `$$` in replacement strings is hardcoded `1` | real PID (deliberate sandbox determinism) | `tests/expansion_transforms.rs` |
| `${v/x/"p\$q"}` re-expands `$q` after unescaping | bash keeps `$` literal | `tests/expansion_transforms.rs` |
| `${arr[@]@Q}` reverses assoc-array elements; `${!r@Q}` collapses to one joined word | bash preserves order/words | `tests/expansion_transforms.rs` |
| `[^]]` does not match `x` | bash: negated class matches non-`]` | `tests/pattern_cov.rs` |
| `[[:a-1:]]` matches literal `[]` *(unverified)* | bash likely rejects | `tests/pattern_cov.rs` |
| extglob nesting depth >64 fails to match | bash has no limit (rust-bash recursion guard) | `tests/pattern_cov.rs` |
| `echo x 2>&1-` discards the line silently | bash errors on write to closed fd | `tests/walker_redirects.rs` |
| `echo x 1<>/missing` fails input collection; file created empty during error handling | bash creates the file and writes | `tests/walker_redirects.rs::readwrite_redirect_missing_file_divergence` |
| `exec {fd}<> /missing` fails before exec runs | bash creates the file | `tests/walker_redirects.rs::exec_fd_variable_alloc_readwrite_missing_file_divergence` |

## 3. Builtins

**Systematic: unknown flags are silently ignored** where bash exits 2 with "invalid option" — `unset -q`, `set -Z`, `readonly -z`, `read -z`, `hash -t`. `command -Z …` instead treats the flag as the command name (exit 127). Pinned in `tests/builtins_cov.rs`.

| Behavior | Expected | Pinned in |
|---|---|---|
| `readonly -` / `declare - x` / `local -` — bare `-` is an invalid variable name | bash: various (usually ignored) | `tests/builtins_cov.rs` |
| `local` outside a function succeeds silently | bash: "can only be used in a function" | `tests/builtins_cov.rs` |
| `declare -A x=plain` creates empty assoc; `local -A x=plain` assigns scalar | bash: "must use subscript" | `tests/builtins_cov.rs` |
| `declare assoc+=(x y)` — subscript-less words silently ignored | bash: error | `tests/builtins_cov.rs` |
| `local assoc+=(…)` propagates an `Execution` error out of `exec` | bash: stderr + exit 1 | `tests/builtins_cov.rs` |
| `local`'s assoc literal parser keeps quotes inside keys (stores `"k x"`) | bash unquotes | `tests/builtins_cov.rs` |
| `dirs +N` prints the whole stack | bash prints only entry N | `tests/builtins_cov.rs` |
| `mapfile -C cb -c n` — callback consumed but never invoked | bash invokes it | `tests/builtins_cov.rs` |
| `read -t` with no value parses as timeout 0, succeeds without reading | bash: invalid timeout | `tests/builtins_cov.rs` |
| `builtin nosuchcmd` on a crafted `# built-in:` stub → 127 via stub path | n/a (edge of the stub mechanism) | `tests/builtins_cov.rs` |

## 4. Text / coreutils commands

**Systematic patterns** (pinned across `tests/fixtures/comparison/text/*.toml`):

- **Silently-ignored flags GNU rejects**: `sort -f/-s` (also wrong order under `-f`), `sort -k` without value, `tr -z`, `tr` single-set, `tr` reversed range, `uniq -z`, `cut -z`, `fmt -x`, `expand/unexpand -t` empty/garbage/0, `tail -n` without value, `basename -a`, `uname -z` / `uname <operand>`.
- **Doubled-path error messages**: `cmd: /path: No such file or directory: /path` (no command prefix, path repeated) — grep-family, base64/sha sums, bc, file, realpath, xargs/find.

| Behavior | Expected | Pinned in |
|---|---|---|
| `tail -n +N` treated as "last N" | GNU: from line N | `text/head_tail_wc.toml` |
| `od` default dumps per-byte octal; `od /dev/zero` = empty stdin | GNU: 2-byte units; endless zeros | `text/od_tr.toml` |
| `rg` implicit-path display `/./t/…`; nonexistent search path silently ignored | GNU rg: `./t/…`; exit 2 | `text/rg.toml` |
| `printf`: empty numeric arg → 0 silently; trailing `\` dropped; `\777` → U+01FF; `%q` emits `é` literally | bash: error/mask to byte/octal-quote | `text/printf.toml` |
| grep missing-arg message includes the dash (`-- '-A'`) | GNU: `-- 'A'` | `text/grep.toml` |
| `join`: garbage `-o` spec skipped; out-of-range join field → empty keys → no output | GNU: error; joins all pairs | `text/comm_join.toml` |
| `du /file` without `-s` prints nothing | GNU prints `1\t/file` | `tests/file_ops_cov.rs` |
| `xargs` treats unknown options as the command name (127) | GNU: invalid option, exit 1 | `tests/exec_cmds_cov.rs` |
| `which ./q` → `/tmp/./q` (unresolved `./` component) | normalized | `tests/cmd_utils_cov.rs` |
| `bc quit` skips the line instead of exiting; f64 arithmetic (`1/3` at scale 20 → `0.33333333333333331483`); `scale` var prints `2.00` | real bc exits; arbitrary precision | `tests/cmd_utils_cov.rs` |

## 5. awk

| Behavior | Expected | Pinned in |
|---|---|---|
| `print "x" > "/f"` parses `>` as comparison (prints `1`); `>>`/`\|` output goes to stdout | real awk redirects (lexer documents "parsed but not fully supported") | `tests/awk_cov.rs` |
| `awk -- '{print}'` → "no program text" | real awk treats next arg as program | `tests/awk_cov.rs` |
| Unknown string escape `"x\qy"` keeps the backslash | gawk strips it with a warning | `tests/awk_cov.rs` |
| Division/modulo by zero → stderr warning, yields `0`, exit 0 | gawk: fatal error | `tests/awk_cov.rs` |
| `1 = 2` (non-lvalue assignment) silently ignored | gawk: parse-time error | `tests/awk_cov.rs` |
| Unknown function → runtime warning, empty value, exit 0 | gawk: parse-time fatal | `tests/awk_cov.rs` |
| Top-level `break` silently aborts the action | gawk: fatal error | `tests/awk_cov.rs` |
| `(a)[1]` parenthesized array-ref accepted | gawk: syntax error | `tests/awk_cov.rs` |
| `index("abc", "")` → 0 *(suspected)* | gawk returns 1 | `tests/awk_cov.rs` |
| `sqrt(-1)` → `nan`, no warning | gawk warns, prints `-nan` | `tests/awk_cov.rs` |
| `sprintf("%+d", 5)` → `5` (flag accepted, ignored) | gawk: `+5` | `tests/awk_cov.rs` |
| `%g` keeps trailing zeros in scientific (`1.23450e-05`) *(suspected)* | C/gawk strip to `1.2345e-05` | `tests/awk_cov.rs` |
| `getline` is a stub returning 0 | real getline | `tests/awk_cov.rs` |

## 6. sed / diff / compression

| Behavior | Expected | Pinned in |
|---|---|---|
| sed: lone `!` is a silent no-op | GNU: exit 1 | `tests/fixtures/comparison/sed/extra.toml` |
| sed: `s/a/b/q` parses `q` as Quit | GNU: "unknown option to s" | sed/extra.toml |
| sed: `\q` in replacement stays literal | GNU collapses | sed/extra.toml |
| sed: branch to undefined label silently ends script | GNU: exit 4 | sed/extra.toml |
| diff: context-format sections without changes omit equal context lines; hunk headers always include explicit counts | GNU prints context | `tests/fixtures/comparison/diff/extra.toml` |
| gzip: "already exists" / "unknown suffix" exit 1 | GNU: exit 2 | `tests/fixtures/comparison/compression/commands.toml` |
| tar: rejects absolute/`..` member names ("error writing", exit 1) | GNU strips them | compression/commands.toml |
| tar: missing input → "No such file or directory", exit 1 | GNU: "Cannot stat", exit 2 | compression/commands.toml |
| tar: `-w` rejected; symlink entries extract as empty regular files | GNU accepts; recreates symlinks | compression/commands.toml |

## 7. VFS semantics (bash and Python share these)

| Behavior | Expected | Pinned in |
|---|---|---|
| `mkdir` through a file component succeeds | POSIX: ENOTDIR | `tests/python_bridge.rs::python_mkdir_through_file_succeeds_like_bash` |
| `rename` file-onto-directory succeeds | POSIX: EISDIR | `tests/python_bridge.rs::python_rename_file_onto_directory_succeeds_like_bash` |
| `InMemoryFs::rename` loses the source node when destination navigation fails (src extracted before dst validation) | atomic rename | `tests/vfs_cov.rs::memory_rename_dst_parent_errors` |
| `OverlayFs::remove_dir("/")` succeeds on an empty merged root and whiteouts `/` | rmdir("/") → EBUSY | `tests/vfs_cov.rs::mkdir_root_after_rmdir_root_reports_already_exists` |
| Overlay glob does not traverse an upper symlink pointing into the lower layer | merged view would | `tests/vfs_cov.rs::glob_through_upper_symlink_to_lower_dir_finds_nothing` |

## 8. Misc

| Behavior | Expected | Pinned in |
|---|---|---|
| `[[ 65#a -eq 5 ]]` / invalid base-N literal → silently 0 | bash: "value too great for base" + exit 1 | `tests/test_cmd_cov.rs` |
| `test -o errtrace` tracks the `errexit` flag; `set -o errtrace` doesn't enable it | bash: distinct `-E` option | `tests/test_cmd_cov.rs` |
| `test foo =~ bar` → false, exit 1 | bash: "binary operator expected", exit 2 | `tests/test_cmd_cov.rs` |
| `jq -n 'infinite'` → `null` (`nan` → `null` matches jq) | real jq: `1.7976931348623157e+308` | `tests/jq_cov.rs` |

## Maintenance

1. New pinned divergences discovered during development must be added here with their pinning test.
2. Fixing a divergence means: behavior change + updated test + removed registry entry, one commit.
3. Section 1 entries are the recommended starting point for fidelity work; `expand -t 0,` is the only known **host-panic** path and should be fixed first.
