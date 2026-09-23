# Chapter 11: Known Divergences

This chapter is the consolidated registry of places where rust-bash's actual behavior diverges from real bash, GNU coreutils, gawk, or POSIX. Every entry is **pinned by a test** (mostly written during the crate-wide coverage campaign) that asserts the *actual* behavior with a `DIVERGENCE` / `pinned` comment.

## Policy

- Divergences are **pinned, never silently fixed**. A behavior change to any entry below requires updating both the pinning test and this registry in the same commit.
- Entries marked *(suspected)* were reasoned from documentation, not verified against a live reference implementation.
- Systematic patterns (same divergence across many commands) are listed once with their command list.
- The registry is descriptive, not a commitment to fix. Entries are candidates for fidelity work, prioritized by how likely an agent-generated script is to trip over them.

## 1. Functional bugs (best fix candidates)

Fix priority, just-bash-style: **Critical** = host crash / wrong output on common agent paths; **High** = visible wrong behavior on realistic scripts; **Medium** = edge-case fidelity; **Low** = cosmetic.

*No open entries.* All section-1 bugs have been fixed; see "Fixed and retired".

### Fixed and retired

- ~~Nested ternary in parens `$(( (1 ? 2 : 0 ? 3 : 4) + 0 ))` → `expected RParen`~~ — fixed by re-architecting arithmetic evaluation: `brush_parser::arithmetic::parse` produces a full expression AST and rust-bash tree-walks it (structural short-circuit — untaken branches are never evaluated), deleting the textual `skip_*` family and its paren-tracking bug class. Test: `tests/arithmetic_eval.rs::nested_ternary_inside_parens_evaluates_correctly`. Two edge notes registered in §2.

- ~~`echo ${ ;}` prints `${;}`; `${ case $x in (a) …; }` and `${ echo hi; }` ksh-style command substitution accepted~~ — fixed: legacy ksh `${ ...; }` / `${|…}` forms are rejected pre-parse with bash's exact surface (exit 1, `rust-bash: {token}: bad substitution`), incl. inside double quotes; single-quoted stays literal. The rewrite machinery was deleted (net −60 lines). One residual difference: because we parse the whole input up front, commands *before* the offending one in the same `exec` do not run, whereas bash parses incrementally and preserves their output (consistent with our handling of all other syntax errors). Tests: `tests/integration.rs` legacy_ksh_*, `tests/interp_core_cov.rs`; oils `command-sub-ksh.test` cases demoted to xfail.
- ~~`expand -t 0,` host panic~~ — fixed: GNU-mirror `-t` validation (`parse_tab_stops`): integer ≥ 1, ascending, GNU's exact error messages, for both `expand` and `unexpand`. `unexpand` list support remains pinned (see §4).
- ~~`format_scientific(inf)` → `"NaNe+2147483647"`~~ — fixed: non-finite guard in both formatter families prints C/glibc `inf`/`INF`/`nan` (gawk's `"+inf"` remains a minor divergence, see §5).

### Refuted entries (verified against real bash 5.2, 2026-06)

These were pinned as suspected divergences during the coverage campaign but real bash behaves exactly as we do:

- `[[ abc == @(a|b)c ]]` (extglob) and the `nocasematch` variant: `@` means "exactly one of", so the pattern expands to `ac`/`bc` and real bash also returns 1. Tests renamed to `extglob_exactly_one_semantics_match_bash` / `nocasematch_extglob_nonmatch_matches_bash`.
- `A=1 [[ x = y ]]`: real bash does not recognize `[[` as a reserved word after an assignment prefix either — `[[: command not found`, exit 127, matching us and just-bash.

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
| `$((08#1))` (base constant with leading zero, non-strict mode) → syntax error | bash accepts (base 8); brush's grammar requires the base to start `[1-9]`. `strict_arith` mode already rejected it | edge, registered |
| Assoc subscripts containing unquoted whitespace (`$((m[k + 1]))`) render compacted (`k+1`) as the key | bash uses the verbatim text (`k + 1`); brush's AST drops whitespace. Quoted keys (`m["k + 1"]`) are exact | `tests/arithmetic_eval.rs` (operator-rich subscript test) |
| `echo x 1<>/missing` fails input collection; file created empty during error handling | bash creates the file and writes | `tests/walker_redirects.rs::readwrite_redirect_missing_file_divergence` |
| `exec {fd}<> /missing` fails before exec runs | bash creates the file | `tests/walker_redirects.rs::exec_fd_variable_alloc_readwrite_missing_file_divergence` |
| `exec {fd}>&2` allocates the fd number but the dup is unsupported; writing to it fails with "Bad file descriptor" | bash dups the fd | `tests/walker_redirects.rs::exec_fd_variable_alloc_dup_output_is_ignored` |
| Cross-class brace ranges: `{z..A}` errors "bad brace expansion"; `{1..a}` / `{z..3}` stay literal | bash expands cross-class ranges by code point | `src/interpreter/brace.rs` (`mixed_case_char_range_errors`, `mixed_numeric_char_ranges_are_literal`) |
| 11-hop arithmetic name indirection bottoms out at 0; the two guards disagree — `value_from_string` errors at depth 10 while `resolve_var_recursive` silently zeroes | bash allows ~1024 hops | `tests/arithmetic_eval.rs::deep_variable_indirection_chain_bottoms_out` |
| `echo hi 10>/f` (fd>2 output redirect without a persistent fd) is silently ignored — the file is never created | bash creates the file | `tests/walker_redirects.rs::high_fd_output_redirect_without_persistent_fd_is_ignored` |
| `cat foo > foo` preserves the content (stdout buffered before truncation); only compound commands / `[[ ]]` pre-truncate (`pre_truncate_output_files`), so `{ cat foo; } > foo` empties it | bash empties the file in both forms | `tests/walker_redirects.rs::self_redirect_simple_command_preserves_content`, `compound_self_redirect_truncates_like_bash` |
| `set -u` arithmetic read of an unset element of a declared-but-empty array (`declare -a a; echo $((a[0]))`) yields 0 (nounset checks only the variable, not the element) | bash: "unbound variable" | `tests/arithmetic_eval.rs::nounset_declared_empty_array_element_reads_zero` |
| Negative shift counts wrap (`$((1 << -1))` masks the count to 63) | bash errors | `tests/arithmetic_eval.rs::negative_shift_count_wraps` |
| Subshell/cmdsubst writes through inherited persistent fds vanish (fs is deep-cloned): `exec > /f; (echo sub); echo main` loses "sub" | bash appends through the shared open file description | `tests/walker_redirects.rs::subshell_write_through_inherited_persistent_fd_vanishes` |

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

- **Silently-ignored flags GNU rejects**: `sort -f/-s` (also wrong order under `-f`), `sort -k` without value, `tr -z`, `tr` single-set, `tr` reversed range, `uniq -z`, `cut -z`, `fmt -x` (and any unknown `fmt` option), `tail -n` without value, `basename -a`, `uname -z` / `uname <operand>`. Also ignored by arg parsers (no comparison fixture; ignore branches in `src/commands/`): `cp` drops all non-`-r/-R` flags (`file_ops.rs`), `mv` and `rm` drop non-`-r/-R/-f` flags, `stat` drops all flags, `cat`, `touch`, and `mkdir` skip any dash-arg (`mod.rs`), `realpath`, `basename`, `dirname`, `tree` skip any dash-arg (`navigation.rs`), `seq` skips non-numeric flags (`utils.rs`), `md5sum`/`sha1sum`/`sha256sum` skip any dash-arg (`utils.rs`), `nl` skips any dash-arg (`text.rs`). (Retired from this list: `expand/unexpand -t` garbage/0 — now GNU-mirrored errors.)
- **Doubled-path error messages**: `cmd: /path: No such file or directory: /path` (no command prefix, path repeated) — grep-family, base64/sha sums, bc, file, realpath, xargs/find.

| Behavior | Expected | Pinned in |
|---|---|---|
| `tail -n +N` treated as "last N" | GNU: from line N | `text/head_tail_wc.toml` |
| `od` default dumps per-byte octal; `od /dev/zero` = empty stdin | GNU: 2-byte units; endless zeros | `text/od_tr.toml` |
| `rg` implicit-path display `/./t/…`; nonexistent search path silently ignored | GNU rg: `./t/…`; exit 2 | `text/rg.toml` |
| `printf`: empty numeric arg → 0 silently; trailing `\` dropped; `\777` → U+01FF; `%q` emits `é` literally | bash: error/mask to byte/octal-quote | `text/printf.toml` |
| grep missing-arg message includes the dash (`-- '-A'`) | GNU: `-- 'A'` | `text/grep.toml` |
| `join`: garbage `-o` spec skipped; out-of-range join field → empty keys → no output | GNU: error; joins all pairs | `text/comm_join.toml` |
| `unexpand -t 2,4` (valid tab-stop list) silently falls back to width 8 | GNU honors the list | `text/expand_unexpand.toml::unexpand_tab_list_falls_back_to_8` |
| `du /file` without `-s` prints nothing | GNU prints `1\t/file` | `tests/file_ops_cov.rs` |
| `xargs` treats unknown options as the command name (127) | GNU: invalid option, exit 1 | `tests/exec_cmds_cov.rs` |
| `xargs` joined flag forms unsupported (`xargs -n1` → `-n1: command not found`); only the separated form (`-n 1`) works | GNU accepts joined forms | `tests/exec_cmds_cov.rs::xargs_joined_flag_form_unsupported` |
| `which ./q` → `/tmp/./q` (unresolved `./` component) | normalized | `tests/cmd_utils_cov.rs` |
| `bc quit` skips the line instead of exiting; f64 arithmetic (`1/3` at scale 20 → `0.33333333333333331483`); `scale` var prints `2.00` | real bc exits; arbitrary precision | `tests/cmd_utils_cov.rs` |

## 5. awk

The BWK conformance suite (`tests/awk_conformance.rs`, 225 programs recorded from gawk 5.0) passes 214/225; the 11 skips are implementation-defined (for-in iteration order, rand() default sequence), policy (pipe forms), or a byte-level `%c` corner — each reverse-asserted in the suite's skip list.

File I/O is implemented: `print`/`printf` `>` (truncate-once) and `>>` through the sandbox fs, `getline` bare/var/`< file` forms with gawk cursor semantics, and `close()`. **Pipe forms (`print | "cmd"`, `"cmd" | getline`) are deliberately unimplemented** pending an explicit security decision (awk's backdoor to command execution); they fail *visibly* (stderr + exit 1), never silently. See `tests/fixtures/comparison/awk/io.toml`.

| Behavior | Expected | Pinned in |
|---|---|---|
| `awk -- '{print}'` → "no program text" | real awk treats next arg as program | `tests/awk_cov.rs` |
| Division/modulo by zero → stderr warning, yields `0`, exit 0 | gawk: fatal error | `tests/awk_cov.rs` |
| `1 = 2` (non-lvalue assignment) silently ignored | gawk: parse-time error | `tests/awk_cov.rs` |
| Unknown function → runtime warning, empty value, exit 0 | gawk: parse-time fatal | `tests/awk_cov.rs` |
| Top-level `break` silently aborts the action | gawk: fatal error | `tests/awk_cov.rs` |
| `(a)[1]` parenthesized array-ref accepted | gawk: syntax error | `tests/awk_cov.rs` |
| `sqrt(-1)` → `nan`, no warning | gawk warns, prints `-nan` | `tests/awk_cov.rs` |
| `sprintf("%+d", 5)` → `5` (flag accepted, ignored) | gawk: `+5` | `tests/awk_cov.rs` |
| `%g` keeps trailing zeros in scientific (`1.23450e-05`) *(suspected)* | C/gawk strip to `1.2345e-05` | `tests/awk_cov.rs` |
| Non-finite floats print libc-style `inf`/`INF` | gawk prints `+inf` | `tests/awk_cov.rs::awk_non_finite_float_formats` |
| Undefined function call `foo(1)` parses as concatenation of the variable `foo` with `(1)` (no spacing info in tokens to enforce gawk's no-space rule) | gawk: parse-time error for undefined functions | `tests/awk_cov.rs::undefined_function_name_parses_as_concatenation` |
| User function named like a builtin (`function length(x)`) shadows the builtin | gawk: parse-time rejection | `tests/awk_cov.rs` |
| awk fatal type-misuse (`attempt to use scalar as array`) prints the message + exit 2 but does NOT abort the run | gawk aborts immediately (END skipped) | `tests/awk_cov.rs` |
| Space between a user-function name and its call paren accepted (`f (1)`) | gawk: rejected (concat ambiguity) | `tests/awk_cov.rs` |
| Field assignments lose no strnum distinction: `$1 = "5"` reads back as a strnum (numeric comparison) since fields carry no per-field attribute | gawk: the assigned string constant is NOT a strnum (string comparison) | none (too deep to pin cheaply; fields are stored as plain strings) |
| Deferred-write blind spot: `print > "/f"` then `getline < "/f"` in one run reads the stale pre-run content (writes are applied after the run finishes) | gawk sees the just-written record | `tests/awk_cov.rs::print_then_getline_same_file_reads_stale_content` |
| Assignment operands (`awk '{print x}' x=1 /f`) misdiagnosed as missing files (exit 2) | gawk applies the assignment when reached in ARGV order | `tests/awk_cov.rs::assignment_operand_misdiagnosed_as_missing_file` |

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
| MountableFs: cross-mount absolute symlinks are stored verbatim in the link's backend and can never resolve on read (`ln -s /real.txt /project/link` where `/real.txt` lives on another mount → reads fail NotFound) | merged view resolves the target | `tests/vfs_cov.rs::mountable_cross_mount_absolute_symlink_never_resolves` |
| MountableFs: `mkdir` at a mount point returns InvalidPath (lookup strips the prefix to the backend root) | AlreadyExists (the mount point exists) | `tests/vfs_cov.rs::mountable_mkdir_at_mount_point_returns_invalid_path` |
| `InMemoryFs::hardlink` / overlay hardlink copy content: later appends through one name are invisible through the other, while `file_id` stays shared | real hard links share content | `tests/vfs_cov.rs::memory_hardlink_copies_content_and_diverges_after_append`, `src/vfs/overlay_tests.rs::hardlink_from_lower` |
| `InMemoryFs::mkdir_p` errors NotADirectory through an existing symlink component | bash follows the symlink | `tests/vfs_cov.rs::memory_mkdir_p_through_symlink_component_errors` |
| `OverlayFs::symlink` doesn't EEXIST-check the merged view (`ln -s t /existing_lower_file` succeeds, shadowing the lower file) | EEXIST | `tests/vfs_cov.rs::overlay::symlink_onto_existing_lower_file_succeeds` |

## 8. Misc

| Behavior | Expected | Pinned in |
|---|---|---|
| `[[ 65#a -eq 5 ]]` / invalid base-N literal → silently 0 | bash: "value too great for base" + exit 1 | `tests/test_cmd_cov.rs` |
| `$RANDOM` sequences after an explicit `RANDOM=N` seed are stable but not bit-identical to bash's sbrand generator (we use xorshift32; semantics — entropy seeding, `RANDOM=N` reseed with arithmetic evaluation, subshell/cmdsubst reseed, 15-bit range — mirror bash 5.2, tested in `tests/random_semantics.rs`) | bit-identical sequences | `tests/random_semantics.rs` |
| `test -o errtrace` tracks the `errexit` flag; `set -o errtrace` doesn't enable it | bash: distinct `-E` option | `tests/test_cmd_cov.rs` |
| `test foo =~ bar` → false, exit 1 | bash: "binary operator expected", exit 2 | `tests/test_cmd_cov.rs` |
| `jq -n 'infinite'` → `null` (`nan` → `null` matches jq) | real jq: `1.7976931348623157e+308` | `tests/jq_cov.rs` |
| `yes` caps at 10,000 lines; `seq` caps at 1M items (architectural: pipelines are buffered — real bash relies on SIGPIPE, which a synchronous in-process pipeline cannot deliver; an unbounded `yes` would never return) | unbounded until killed by SIGPIPE | `src/commands/utils.rs` |
| Limit trips inside awk/sed/jq surface as `Err(LimitExceeded)` (guardrail event), losing partial output | bash has no limits; a harness-killed process yields partial output | `tests/awk_cov.rs`, `tests/filecmds_cov.rs`, `tests/jq_cov.rs` |

## Maintenance

1. New pinned divergences discovered during development must be added here with their pinning test.
2. Fixing a divergence means: behavior change + updated test + removed registry entry, one commit.
3. Section 1 entries are the recommended starting point for fidelity work.
