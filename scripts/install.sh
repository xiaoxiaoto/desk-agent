#!/bin/sh
# Build and install DeskAgent, then run install-time authorization once:
# request macOS permissions and record standing grants for every capability.
# Usage: scripts/install.sh [PREFIX]   (default PREFIX: $HOME/.local)
set -eu
PREFIX="${1:-$HOME/.local}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cargo build --release --manifest-path "$ROOT/Cargo.toml"
mkdir -p "$PREFIX/bin"
for bin in desk-agent desk-agent-cli desk-agent-desktop; do
    install -m 0755 "$ROOT/target/release/$bin" "$PREFIX/bin/$bin"
done
"$PREFIX/bin/desk-agent" setup
echo "Installed to $PREFIX/bin. Revoke standing grants with: desk-agent setup --revoke"
