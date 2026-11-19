#!/bin/sh
# Version checks for the release.
#
#   scripts/check-versions.sh              fail unless Cargo.toml, herdr-plugin.toml and the top
#                                          CHANGELOG.md entry agree on the version
#   scripts/check-versions.sh --notes V    print the CHANGELOG.md section of version V (without
#                                          its heading), for the GitHub release body; fails when
#                                          there is no such section or it is empty
set -eu

cd "$(dirname "$0")/.."

notes() {
    version=${1#v}
    case $version in
        '' | *[!0-9.]*)
            echo "check-versions: not a version: $1" >&2
            exit 2
            ;;
    esac
    section=$(awk -v want="$version" '
        /^## / {
            if (inside) exit
            heading = substr($0, 4)
            sub(/^\[/, "", heading)
            sub(/[] ].*$/, "", heading)
            if (heading == want) { inside = 1; found = 1 }
            next
        }
        inside && /^\[[^]]+\]: / { next }
        inside { print }
        END { exit found ? 0 : 1 }
    ' CHANGELOG.md) || {
        echo "check-versions: CHANGELOG.md has no section for $version" >&2
        exit 1
    }
    # Trim blank lines at both ends.
    section=$(printf '%s\n' "$section" | sed -e '/./,$!d' | sed -e ':a' -e '/^[[:space:]]*$/{$d;N;ba' -e '}')
    if [ -z "$section" ]; then
        echo "check-versions: the CHANGELOG.md section for $version is empty" >&2
        exit 1
    fi
    printf '%s\n' "$section"
}

case ${1:-} in
    --notes)
        [ $# -eq 2 ] || {
            echo "usage: scripts/check-versions.sh [--notes VERSION]" >&2
            exit 2
        }
        notes "$2"
        exit 0
        ;;
    '') ;;
    *)
        echo "usage: scripts/check-versions.sh [--notes VERSION]" >&2
        exit 2
        ;;
esac

cargo_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
plugin_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' herdr-plugin.toml | head -n 1)
changelog_version=$(sed -n 's/^## \[\{0,1\}\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\).*$/\1/p' CHANGELOG.md | head -n 1)

status=0
if [ "$cargo_version" != "$plugin_version" ]; then
    echo "version mismatch: Cargo.toml=$cargo_version herdr-plugin.toml=$plugin_version" >&2
    status=1
fi
if [ "$cargo_version" != "$changelog_version" ]; then
    echo "version mismatch: Cargo.toml=$cargo_version CHANGELOG.md=$changelog_version" >&2
    status=1
fi
if [ "$status" -eq 0 ]; then
    echo "version $cargo_version"
fi
exit "$status"
