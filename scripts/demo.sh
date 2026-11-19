#!/bin/sh
# Record the picker demo (docs/demo.tape) into docs/assets/picker.gif and picker.png.
# Builds a static binary (the VHS image has an older glibc than the host), then runs VHS in a
# container with that binary on PATH. Uses podman, else docker.
set -eu

cd "$(dirname "$0")/.."

IMAGE=ghcr.io/charmbracelet/vhs:v0.12.1
TARGET=x86_64-unknown-linux-musl

runtime=
for rt in podman docker; do
    if command -v "$rt" >/dev/null 2>&1 && "$rt" info >/dev/null 2>&1; then
        runtime=$rt
        break
    fi
done
if [ -z "$runtime" ]; then
    echo "demo.sh: neither podman nor docker is usable" >&2
    exit 1
fi

rustup target add "$TARGET"
cargo build --release --locked --target "$TARGET"

mkdir -p docs/assets
"$runtime" run --rm \
    -v "$PWD":/vhs:Z \
    -v "$PWD/target/$TARGET/release":/opt/gliffherdr:ro,Z \
    -w /vhs \
    "$IMAGE" docs/demo.tape

# The container runs as another uid: make the files ordinary again.
chmod 644 docs/assets/picker.gif docs/assets/picker.png
ls -l docs/assets/picker.gif docs/assets/picker.png
