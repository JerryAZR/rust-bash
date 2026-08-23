#!/usr/bin/env bash
# Fetch the CPython wasm32-wasip1 artifact used by the Python bridge
# examples/tests. The artifact is a frozen 3.12.0 single-file build
# (stdlib embedded) from vmware-labs/webassembly-language-runtimes.
#
# Primary source: mirror release on this fork (the upstream repo is being
# archived, so its release URLs may rot). Fallback: the upstream release.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST_DIR="$(cd "$SCRIPT_DIR/.." && pwd)/third_party/python-wasm"
DEST="$DEST_DIR/python-3.12.0.wasm"

MIRROR_URL="https://github.com/JerryAZR/rust-bash/releases/download/python-wasm-3.12.0/python-3.12.0.wasm"
UPSTREAM_URL="https://github.com/vmware-labs/webassembly-language-runtimes/releases/download/python%2F3.12.0%2B20231211-040d5a6/python-3.12.0.wasm"

EXPECTED=$(cut -d' ' -f1 "$DEST_DIR/python-3.12.0.wasm.sha256sum")

if [ -f "$DEST" ] && echo "$EXPECTED  $DEST" | sha256sum -c - >/dev/null 2>&1; then
    echo "python-3.12.0.wasm already present and verified."
    exit 0
fi

mkdir -p "$DEST_DIR"
for url in "$MIRROR_URL" "$UPSTREAM_URL"; do
    echo "Downloading $url"
    if curl -fL --retry 3 -o "$DEST.tmp" "$url" || curl -fL --ssl-no-revoke --retry 3 -o "$DEST.tmp" "$url"; then
        if echo "$EXPECTED  $DEST.tmp" | sha256sum -c - >/dev/null 2>&1; then
            mv "$DEST.tmp" "$DEST"
            echo "OK: $DEST (sha256 verified)"
            exit 0
        fi
        echo "sha256 mismatch for download from $url" >&2
    fi
    rm -f "$DEST.tmp"
done

# Fallback for environments where curl's TLS stack is restricted but the
# GitHub CLI is authenticated. (The --ssl-no-revoke retry above fires on ANY
# curl failure, not just revocation errors — acceptable because the sha256
# check below gates every acquisition path. Note: sha256sum is GNU coreutils;
# macOS contributors need `shasum -a 256` or coreutils installed.)
if command -v gh >/dev/null 2>&1; then
    echo "Trying gh release download"
    if gh release download python-wasm-3.12.0 --repo JerryAZR/rust-bash \
        --pattern python-3.12.0.wasm --dir "$DEST_DIR" --clobber \
        && echo "$EXPECTED  $DEST" | sha256sum -c - >/dev/null 2>&1; then
        echo "OK: $DEST (sha256 verified)"
        exit 0
    fi
fi

echo "ERROR: could not obtain a valid python-3.12.0.wasm" >&2
exit 1
