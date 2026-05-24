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
# static assets (see the `assets` block in wrangler.jsonc).
if ! command -v trunk >/dev/null 2>&1; then
  echo "Installing trunk..."
  cargo install -q trunk --version 0.21.14 --locked
fi
(cd frontend && trunk build --release)

cd workers-rssfilter && worker-build --release
