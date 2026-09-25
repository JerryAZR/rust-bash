#!/usr/bin/env bash
# Record BWK awk conformance expectations from a reference awk (gawk).
# Dev-time only; re-run deliberately. Output: tests/fixtures/awk_conformance/bwk/expected.toml
set -u
export PYTHONIOENCODING=utf-8
# Locale-sensitive behaviors (collation in string comparison, case ops)
# must record under the C locale for reproducibility.
export LC_ALL=C

FIXTURES="tests/fixtures/awk_conformance/bwk"
OUT="$FIXTURES/expected.toml"
AWK_BIN="${AWK_BIN:-/usr/bin/awk}"

# Require gawk specifically: BWK awk / mawk have materially different
# behaviors (e.g. function-name rules, getline edge cases) and recording
# from the wrong reference silently poisons the suite.
if ! "$AWK_BIN" --version 2>/dev/null | head -1 | grep -q "GNU Awk"; then
    echo "error: $AWK_BIN is not gawk (GNU Awk) — set AWK_BIN to a gawk binary" >&2
    exit 1
fi
# python is required for TOML escaping below.
if ! command -v python >/dev/null 2>&1; then
    echo "error: python is required for TOML escaping" >&2
    exit 1
fi
# Never leave a partial expected.toml behind on interruption.
trap 'rm -f "$OUT.partial"' EXIT
echo "# Recorded from: $($AWK_BIN --version | head -1) (LC_ALL=C) -- do not hand-edit" > "$OUT.partial"

record() {
    local prog="$1"; shift
    local stdout_file stderr_file rc
    stdout_file=$(mktemp); stderr_file=$(mktemp)
    # Run from the fixture dir with relative paths so FILENAME/ARGV in the
    # recorded output match what the in-process runner passes.
    (cd "$FIXTURES" && timeout 10 "$AWK_BIN" -f "$prog" "$@") >"$stdout_file" 2>"$stderr_file"
    rc=$?
    python - "$prog" "$rc" "$stdout_file" "$stderr_file" >> "$OUT.partial" <<'PYEOF'
import sys
name, rc, out_f, err_f = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
out = open(out_f, 'rb').read().decode('utf-8', 'replace')
err = open(err_f, 'rb').read().decode('utf-8', 'replace')
# Normalize: gawk prefixes messages with its own name and the -f path.
err = err.replace('gawk:', 'awk:')
import re
err = re.sub(r'tests/fixtures/awk_conformance/bwk/[^\s:]+', '<prog>', err)
def toml_str(s):
    parts = []
    for ch in s:
        o = ord(ch)
        if ch == '"': parts.append('\\"')
        elif ch == '\\': parts.append('\\\\')
        elif ch == '\n': parts.append('\\n')
        elif ch == '\t': parts.append('\\t')
        elif ch == '\r': parts.append('\\r')
        elif o < 32 or o == 127: parts.append(f'\\u{o:04X}')
        else: parts.append(ch)
    return '"' + ''.join(parts) + '"'
print(f'\n[[cases]]\nname = "{name}"')
print(f'exit_code = {rc}')
print(f'stdout = {toml_str(out)}')
print(f'stderr = {toml_str(err)}')
PYEOF
    rm -f "$stdout_file" "$stderr_file"
}

for prog in "$FIXTURES"/p.*; do
    record "$(basename "$prog")" test.countries test.countries
done
for prog in "$FIXTURES"/t.*; do
    record "$(basename "$prog")" test.data
done
mv "$OUT.partial" "$OUT"
trap - EXIT
echo "recorded $(grep -c '\[\[cases\]\]' "$OUT") cases into $OUT"
