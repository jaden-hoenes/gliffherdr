#!/bin/sh
# Licence, advisory, ban and source check (deny.toml) with cargo-deny 0.21.0.
#
#   scripts/deny.sh [cargo-deny check arguments]    default: check
#
# Uses cargo-deny from PATH; otherwise builds the pinned version once into
# /tmp/gliffherdr-deny-cargo (its own CARGO_HOME, so your cargo cache is not touched) and reuses
# it. Advisories are fetched live, so this needs network access. CI runs the same check through
# EmbarkStudios/cargo-deny-action; scripts/gate.sh does not run it.
set -eu

cd "$(dirname "$0")/.."

version=0.21.0
scratch=/tmp/gliffherdr-deny-cargo

if command -v cargo-deny >/dev/null 2>&1; then
    deny=$(command -v cargo-deny)
    home_override=
else
    deny=$scratch/bin/cargo-deny
    home_override=$scratch
    if [ ! -x "$deny" ]; then
        echo "deny: cargo-deny is not on PATH; installing $version into $scratch" >&2
        CARGO_HOME=$scratch cargo install cargo-deny --version "$version" --locked
    fi
fi

if [ "$#" -eq 0 ]; then
    set -- check
fi
if [ -n "$home_override" ]; then
    CARGO_HOME=$home_override exec "$deny" "$@"
fi
exec "$deny" "$@"
