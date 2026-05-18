#!/usr/bin/env bash
# Build all patches-bundles cdylibs in release mode and install them as
# .pxm files into the default bundle scan dir resolved by
# `patches_ffi::scanner::default_bundle_dir` (ADR 0075).
#
# Resolution per OS:
#   macOS : ~/Library/Application Support/Patches/bundles
#   Linux : ${XDG_DATA_HOME:-$HOME/.local/share}/Patches/bundles
#
# Override with PATCHES_BUNDLE_DIR=/path/to/dir.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

case "$(uname -s)" in
    Darwin) default_dest="$HOME/Library/Application Support/Patches/bundles" ;;
    Linux)  default_dest="${XDG_DATA_HOME:-$HOME/.local/share}/Patches/bundles" ;;
    *)      echo "unsupported OS: $(uname -s)" >&2; exit 2 ;;
esac

DEST="${PATCHES_BUNDLE_DIR:-$default_dest}"

echo "==> build + stage via xtask"
(cd "$ROOT" && cargo xtask package)

SRC="$ROOT/release/plugins"
if ! compgen -G "$SRC/*.pxm" > /dev/null; then
    echo "no .pxm artefacts in $SRC" >&2
    exit 1
fi

mkdir -p "$DEST"
echo "==> install to $DEST"
for f in "$SRC"/*.pxm; do
    cp -v "$f" "$DEST/"
done

echo "done. installed $(ls "$DEST"/*.pxm | wc -l | tr -d ' ') bundle(s) in $DEST"
