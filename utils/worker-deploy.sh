#!/bin/bash

# This command is run by wrangler as the build command. It should be run in the
# project root.

set -euxo pipefail

SCRIPT_DIR="$(dirname "${BASH_SOURCE[0]}")"

# Ensure Cargo is installed, on the `PATH`, and the project is built
"${SCRIPT_DIR}/worker-build.sh"
export PATH="${HOME}/.cargo/bin:${PATH}"

cargo install -q worker-build

cd workers-rssfilter && worker-build --release
