#!/bin/sh
# Development install: build the debug binary, point bin/gliffherdr at it and link this checkout
# into herdr. `--no-link` stops after the symlink (used by tests and CI).
set -eu

cd "$(dirname "$0")/.."

LINK=1
for arg in "$@"; do
    case $arg in
        --no-link) LINK=0 ;;
        *)
            printf 'usage: scripts/dev-link.sh [--no-link]\n' >&2
            exit 2
            ;;
    esac
done

cargo build
mkdir -p bin
ln -sf ../target/debug/gliffherdr bin/gliffherdr
printf 'built target/debug/gliffherdr and linked bin/gliffherdr -> ../target/debug/gliffherdr\n'

if [ "$LINK" = 1 ]; then
    "${HERDR_BIN_PATH:-herdr}" plugin link "$PWD"
    printf 'linked %s into herdr\n' "$PWD"
else
    printf 'not linking into herdr (--no-link)\n'
fi
