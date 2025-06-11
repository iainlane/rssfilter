#!/bin/bash

# Install tools to build the project in Cloudflare's build environment.

set -euxo pipefail

if ! command -v cargo 2>/dev/null; then
  echo "Installing Rust and Cargo..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable

  export PATH="${HOME}/.cargo/bin:${PATH}"
fi

echo "Verifying Rust installation..."
rustc --version
cargo --version

rustup target add wasm32-unknown-unknown

if ! command -v wasm-pack 2>/dev/null; then
  echo "Installing wasm-pack..."
  curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh
fi

echo "Building workers-rssfilter crate..."
cargo build --release -p workers-rssfilter --target wasm32-unknown-unknown
