#!/bin/bash

# This command is run by wrangler as the build command. It should be run in the
# project root.

set -euxo pipefail

SCRIPT_DIR="$(dirname "${BASH_SOURCE[0]}")"

# Ensure Cargo is installed, on the `PATH`, and the project is built
"${SCRIPT_DIR}/worker-build.sh"
export PATH="${HOME}/.cargo/bin:${PATH}"

cargo install -q worker-build

# Build the Leptos single-page app into frontend/dist, which wrangler serves as
# static assets (see the `assets` block in wrangler.jsonc). Prefer the prebuilt
# trunk release tarball (seconds) over `cargo install` (compiles from source).
TRUNK_VERSION="0.21.14"
if ! command -v trunk >/dev/null 2>&1; then
  echo "Installing trunk ${TRUNK_VERSION}..."
  curl -L --proto '=https' --tlsv1.2 -sSf \
    "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-x86_64-unknown-linux-gnu.tar.gz" \
    | tar -xz -C "${HOME}/.cargo/bin"
fi
(cd frontend && trunk build --release)

cd workers-rssfilter && worker-build --release
