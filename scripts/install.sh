#!/bin/sh
# Install the gliffherdr binary into bin/. herdr runs this from the plugin root on
# `herdr plugin install` (the manifest's [[build]] command).
#
# Prebuilt: download the release tarball for the manifest version and check its SHA-256. This is
#           the default. When anonymous download fails (a private repository), the logged-in gh
#           CLI is tried. When the release asset is still missing or fails verification and cargo
#           is installed, falls back to a source build; without cargo it fails.
# Source:   cargo build --release --locked.
#
# Environment:
#   GLIFFHERDR_BUILD             source | prebuilt   force a mode
#   GLIFFHERDR_RELEASE_BASE_URL  replaces https://github.com/jaden-hoenes/gliffherdr/releases/download
#                                (gh is then not used unless GLIFFHERDR_GH=1)
#   GLIFFHERDR_GH                1 | 0               force or forbid the gh fallback
set -eu

REPO=jaden-hoenes/gliffherdr
BASE_URL=${GLIFFHERDR_RELEASE_BASE_URL:-https://github.com/$REPO/releases/download}

cd "$(dirname "$0")/.."

say() { printf 'gliffherdr: %s\n' "$*" >&2; }

VERSION=$(sed -n 's/^version = "\(.*\)"[[:space:]]*$/\1/p' herdr-plugin.toml | head -n 1)
if [ -z "$VERSION" ]; then
    say "cannot read the version from herdr-plugin.toml"
    exit 1
fi

# Architecture: anything but x86_64 and aarch64 has no prebuilt binary.
ARCH=
case $(uname -m) in
    x86_64) ARCH=x86_64 ;;
    aarch64 | arm64) ARCH=aarch64 ;;
esac

have() { command -v "$1" >/dev/null 2>&1; }

# Mode.
MODE=
case ${GLIFFHERDR_BUILD:-} in
    source) MODE=source ;;
    prebuilt) MODE=prebuilt ;;
    '') ;;
    *)
        say "GLIFFHERDR_BUILD must be source or prebuilt, not ${GLIFFHERDR_BUILD}"
        exit 1
        ;;
esac
# By default the prebuilt binary of the manifest's version is installed, with or without cargo:
# `herdr plugin install owner/repo` installs the default branch, which is rarely exactly a tag, so
# comparing the checkout with the tag would compile on almost every install. Source is the
# fallback when that binary cannot be had (below) and the only choice without one for this
# architecture.
if [ -z "$MODE" ]; then
    if [ -z "$ARCH" ]; then
        MODE=source
    else
        MODE=prebuilt
    fi
fi
if [ "$MODE" = prebuilt ] && [ -z "$ARCH" ]; then
    say "no prebuilt binary for $(uname -m); building from source"
    MODE=source
fi

TMP=
cleanup() { [ -n "$TMP" ] && rm -rf "$TMP"; return 0; }
trap cleanup EXIT HUP INT TERM

# download URL DEST: curl, else wget.
download() {
    if have curl; then
        curl -fsSL -o "$2" "$1"
    elif have wget; then
        wget -qO "$2" "$1"
    else
        return 1
    fi
}

# Whether to try the gh CLI when the anonymous download fails: a private repository's release
# assets need a login. Only for the real GitHub URL, unless GLIFFHERDR_GH=1 forces it (tests).
use_gh() {
    have gh || return 1
    case ${GLIFFHERDR_GH:-} in
        1) return 0 ;;
        0) return 1 ;;
    esac
    [ -z "${GLIFFHERDR_RELEASE_BASE_URL:-}" ]
}

# fetch_assets TARBALL: put TARBALL and SHA256SUMS of the release into $TMP.
fetch_assets() {
    if download "$BASE_URL/v$VERSION/$1" "$TMP/$1" &&
        download "$BASE_URL/v$VERSION/SHA256SUMS" "$TMP/SHA256SUMS"; then
        return 0
    fi
    say "could not download $1 and SHA256SUMS from $BASE_URL/v$VERSION"
    if use_gh; then
        say "trying gh release download (a private repository needs your gh login)"
        rm -f "$TMP/$1" "$TMP/SHA256SUMS"
        if gh release download "v$VERSION" --repo "$REPO" --pattern "$1" \
            --pattern SHA256SUMS --dir "$TMP" >/dev/null 2>&1; then
            return 0
        fi
        say "gh release download failed"
    fi
    return 1
}

# Download, verify and unpack the prebuilt binary into bin/. Returns non-zero on any failure.
install_prebuilt() {
    tarball=gliffherdr-$VERSION-$ARCH-linux-musl.tar.gz
    TMP=$(mktemp -d)
    fetch_assets "$tarball" || return 1
    # The line for this tarball must be there: --ignore-missing alone would accept a file
    # that lists nothing we downloaded.
    if ! grep -E "[[:space:]]\\*?$tarball\$" "$TMP/SHA256SUMS" >/dev/null 2>&1; then
        say "SHA256SUMS has no entry for $tarball"
        return 1
    fi
    if ! (cd "$TMP" && sha256sum -c --ignore-missing SHA256SUMS >/dev/null 2>&1); then
        say "checksum mismatch for $tarball"
        return 1
    fi
    if ! tar -xzf "$TMP/$tarball" -C "$TMP" gliffherdr; then
        say "$tarball does not contain gliffherdr"
        return 1
    fi
    mkdir -p bin
    cp "$TMP/gliffherdr" bin/gliffherdr.new
    chmod 755 bin/gliffherdr.new
    mv -f bin/gliffherdr.new bin/gliffherdr
}

install_source() {
    if ! have cargo; then
        say "cargo is not installed. Install Rust (https://rustup.rs) and run herdr plugin install again."
        exit 1
    fi
    cargo build --release --locked
    mkdir -p bin
    cp target/release/gliffherdr bin/gliffherdr.new
    chmod 755 bin/gliffherdr.new
    mv -f bin/gliffherdr.new bin/gliffherdr
}

HOW=
if [ "$MODE" = prebuilt ]; then
    if install_prebuilt; then
        HOW=prebuilt
    elif have cargo; then
        say "falling back to a source build"
        MODE=source
    else
        say "could not download the v$VERSION binary and cargo is not installed. Install Rust (https://rustup.rs) and run herdr plugin install again."
        exit 1
    fi
fi
if [ "$MODE" = source ]; then
    install_source
    HOW="built from source"
fi

printf 'gliffherdr %s: installed bin/gliffherdr (%s)\n' "$VERSION" "$HOW"
