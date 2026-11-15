#!/bin/bash

# This command is run by wrangler as the build command. It should be run in the
# project root.

set -euxo pipefail

SCRIPT_DIR="$(dirname "${BASH_SOURCE[0]}")"

# Ensure Cargo is installed, on the `PATH`, and the project is built
"${SCRIPT_DIR}/worker-build.sh"
export PATH="${HOME}/.cargo/bin:${PATH}"

# worker-build refuses to build against a `worker` crate from a different
# minor series, so install the release matching the version in Cargo.lock. That
# way, bumping `worker` (which Renovate does) moves worker-build along with it,
# rather than an unpinned install picking up a new release that doesn't match.
worker_pkgid="$(cargo pkgid worker)"
worker_version="${worker_pkgid##*[#@]}"
cargo install -q --locked worker-build --version "=${worker_version}"

cd workers-rssfilter && worker-build --release
