#!/bin/sh
# Builds the guest agent in release for every target below and installs it as
# mcp-resources/qemu/agent-<triple>.
#
# The server resolves that folder from its working directory, so the same copy serves a
# debug and a release build of mcp-server.
set -eu

TARGETS="aarch64-unknown-linux-musl"
RESOURCE_DIR="mcp-resources/qemu"

cd "$(dirname "$0")"

mkdir -p "$RESOURCE_DIR"

for target in $TARGETS; do
    if ! rustup target list --installed | grep -qx "$target"; then
        echo "installing rust target $target"
        rustup target add "$target"
    fi

    echo "building mcp-qemu-agent for $target"
    cargo build --release --package mcp-qemu-agent --target "$target"

    cp "target/$target/release/mcp-qemu-agent" "$RESOURCE_DIR/agent-$target"
    chmod +x "$RESOURCE_DIR/agent-$target"
    echo "  -> $RESOURCE_DIR/agent-$target"
done
