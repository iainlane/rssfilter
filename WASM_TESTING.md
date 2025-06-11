# WASM Testing Setup

Our primary target is CloudFlare Workers, where we run our code in a WASM
environment. It's a bit different.

## Prerequisites

- `wasm-bindgen-test-runner` must be installed (automatically handled by the
  cargo configuration)
- WASM target must be installed: `rustup target add wasm32-unknown-unknown`

## WASM is a different platform to native Rust

WASM makes your Rust code run on top of the web platform, which means that
various web APIs and features are available, but some native Rust features may
not be. For example, reading from the filesystem won't work as expected, time
isn't available natively, and accessing URLs goes through the web platform's
fetch API.

Many crates transparently support WASM, but not all. This project has extensive
conditional compilation because of these differences, since we are trying (for
some reason) to support both native and WASM targets in the same codebase.

## Running WASM Tests

### Command Line

Run WASM tests for specific crates:

```bash
# Test workers-rssfilter crate
cargo test --target wasm32-unknown-unknown -p workers-rssfilter

# Test filter-rss-feed crate
cargo test --target wasm32-unknown-unknown -p filter-rss-feed

# Test all WASM-compatible crates
cargo test --target wasm32-unknown-unknown
```

### VS Code Integration

Under "Tasks: Run Task", there are tasks for running WASM tests, which start
with WASM.

#### Workspace

Open the `.vscode/{native,wasm32}.code-workspace` files to develop in native or
wasm mode. You'll see that the code for the inactive target is greyed out.

## Configuration

### Cargo Configuration

[`.cargo/config.toml`](.cargo/config.toml) configures `wasm-bindgen-test-runner`
as the test runner for WASM targets.
