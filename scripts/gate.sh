#!/bin/sh
# The definition-of-done gate: every step in order, each printed as
# a header, stopping at the first failure and naming the step.
#
#   scripts/gate.sh              all twelve steps
#   scripts/gate.sh --skip-e2e   skips steps 6 and 7 (the real-sshd and real-herdr suites) for a
#                                quick local run when sshd or herdr is not at hand. The result is
#                                NOT the definition of done; CI and the release pass never use it.
#
# Environment: GLIFFHERDR_HERDR_BIN names the herdr binary for step 7 (default: herdr on PATH).
set -eu

cd "$(dirname "$0")/.."

skip_e2e=0
case ${1:-} in
    '') ;;
    --skip-e2e) skip_e2e=1 ;;
    *)
        echo "usage: scripts/gate.sh [--skip-e2e]" >&2
        exit 2
        ;;
esac

current="(before the first step)"
on_exit() {
    status=$?
    if [ "$status" -ne 0 ]; then
        printf '\ngate: FAILED at %s\n' "$current" >&2
    fi
}
trap on_exit EXIT

step() {
    current="step $1: $2"
    printf '\n==> %s\n' "$current"
}

# Run shellcheck from PATH, else from the pinned container image (podman, else docker).
run_shellcheck() {
    if command -v shellcheck >/dev/null 2>&1; then
        shellcheck "$@"
        return
    fi
    for runtime in podman docker; do
        if command -v "$runtime" >/dev/null 2>&1 && "$runtime" info >/dev/null 2>&1; then
            "$runtime" run --rm -v "$PWD":/mnt:ro,Z -w /mnt \
                docker.io/koalaman/shellcheck:v0.11.0 "$@"
            return
        fi
    done
    echo "gate: shellcheck is not on PATH and neither podman nor docker answers" >&2
    return 1
}

step 1 "scripts/vendor.sh"
scripts/vendor.sh

step 2 "cargo fmt --check"
cargo fmt --check

step 3 "cargo clippy --all-targets --locked -- -D warnings"
cargo clippy --all-targets --locked -- -D warnings

step 4 "cargo doc --no-deps --locked (warnings denied)"
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked

step 5 "cargo test --locked"
INSTA_UPDATE=no cargo test --locked

if [ "$skip_e2e" -eq 1 ]; then
    step 6 "e2e_ssh (SKIPPED by --skip-e2e)"
    step 7 "e2e_herdr (SKIPPED by --skip-e2e)"
    echo "gate: steps 6 and 7 skipped; this run is not the definition of done" >&2
else
    step 6 "e2e_ssh (real sshd)"
    GLIFFHERDR_E2E_SSH=1 cargo test --locked --test e2e_ssh

    step 7 "e2e_herdr (real herdr)"
    herdr_bin=${GLIFFHERDR_HERDR_BIN:-$(command -v herdr || true)}
    if [ -z "$herdr_bin" ]; then
        echo "gate: no herdr binary: set GLIFFHERDR_HERDR_BIN or put herdr on PATH" >&2
        exit 1
    fi
    GLIFFHERDR_E2E_HERDR=1 GLIFFHERDR_HERDR_BIN="$herdr_bin" cargo test --locked --test e2e_herdr
fi

step 8 "shellcheck scripts/*.sh src/check/remote.sh"
run_shellcheck scripts/*.sh src/check/remote.sh

step 9 "scripts/check-versions.sh"
scripts/check-versions.sh

step 10 "no TODO, FIXME, todo!(, unimplemented!(, dbg!( or \"not built yet\" in src/"
if grep -rnE 'TODO|FIXME|todo!\(|unimplemented!\(|dbg!\(|not built yet' src; then
    echo "gate: leftover markers in src/ (listed above)" >&2
    exit 1
fi

step 11 "cargo build --release --locked"
cargo build --release --locked

step 12 "git status --porcelain is empty"
if [ -n "$(git status --porcelain)" ]; then
    git status --short >&2
    echo "gate: the working tree is not clean" >&2
    exit 1
fi

printf '\ngate: all steps passed'
if [ "$skip_e2e" -eq 1 ]; then
    printf ' (e2e skipped)'
fi
printf '\n'
