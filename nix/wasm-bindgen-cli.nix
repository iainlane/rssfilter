{
  # Builds wasm-bindgen-cli at the exact version of the `wasm-bindgen` crate
  # locked in the workspace Cargo.lock. Reading the version from the lockfile
  # means the CLI / test runner can never drift out of sync with the crate — a
  # mismatch otherwise makes the wasm tests fail with a schema-version error.
  # Renovate bumps `wasm-bindgen` through the normal cargo manager and the
  # version here follows automatically.
  #
  # The hashes below are content hashes Renovate cannot recompute. When the crate
  # is bumped this build fails with the expected hashes (a loud, transient
  # failure rather than a silent mismatch); regenerate them with:
  #   nix build .#checks.x86_64-linux.tests-wasm
  # and copy the `got: sha256-...` values in.
  #
  # Temporary local copy of https://github.com/NixOS/nixpkgs/pull/496279.
  # Drop this once the change lands in nixpkgs-unstable.
  lib,
  buildWasmBindgenCli,
  fetchCrate,
  rustPlatform,
}: let
  cargoLock = fromTOML (builtins.readFile ../Cargo.lock);
  wasmBindgen = lib.findFirst (p: p.name == "wasm-bindgen") null cargoLock.package;
  version =
    if wasmBindgen == null
    then throw "wasm-bindgen not found in Cargo.lock"
    else wasmBindgen.version;
in
  buildWasmBindgenCli rec {
    src = fetchCrate {
      pname = "wasm-bindgen-cli";
      inherit version;
      hash = "sha256-vO4RSxi/sMWxmsEs3GuljdMfIRSu75A+Q+c5wgYToRU=";
    };

    cargoDeps = rustPlatform.fetchCargoVendor {
      inherit src;
      inherit (src) pname version;
      hash = "sha256-Inup6vvJSG5ghNyeDPyZbfZo4d0LsMG2OJfStoaeDBs=";
    };
  }
