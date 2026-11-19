#!/bin/sh
# Fetch the reference sources (herdr, gliff) at their pinned commits into .vendor/.
# Idempotent: an existing checkout whose HEAD is already the pinned commit is left alone.
set -eu

cd "$(dirname "$0")/.."

HERDR_URL=https://github.com/herdrdev/herdr.git
HERDR_SHA=2ecf19a92fc3a7fff75b6ce3655ef04f4625ec01
GLIFF_URL=https://github.com/omacom/gliff.git
GLIFF_SHA=9d93732ae3fd4319e071feeecf38ab2cfc19edfc

vendor() {
    name=$1 url=$2 sha=$3
    dir=.vendor/$name
    if [ ! -e "$dir/.git" ]; then
        mkdir -p "$dir"
        git -C "$dir" init -q
        git -C "$dir" remote add origin "$url"
    fi
    head=$(git -C "$dir" rev-parse HEAD 2>/dev/null || true)
    if [ "$head" != "$sha" ]; then
        if ! git -C "$dir" cat-file -e "$sha^{commit}" 2>/dev/null; then
            git -C "$dir" fetch -q --depth 1 origin "$sha"
        fi
        git -C "$dir" checkout -q --detach "$sha"
    fi
    printf '%s %s\n' "$name" "$(git -C "$dir" rev-parse HEAD)"
}

vendor herdr "$HERDR_URL" "$HERDR_SHA"
vendor gliff "$GLIFF_URL" "$GLIFF_SHA"
