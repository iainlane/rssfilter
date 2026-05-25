{
  # Builds the wasm-bindgen-test-runner binary with crane.
  #
  # nix/wasm-bindgen-cli/ is a small "pin" crate whose only job is to let
  # Renovate track wasm-bindgen-cli's version and crate checksum (in lockstep
  # with the wasm-bindgen crate, via the "wasm-bindgen and cloudflare-workers"
  # group). We read just those two fields from its lockfile below.
  #
  # downloadCargoPackage then fetches the crate from crates.io as a fixed-output
  # derivation keyed on that checksum (so there are no Nix hashes to maintain),
  # and crane builds it as the package — installing the runner through its normal
  # hook and vendoring dependencies from the crate's *own* published Cargo.lock.
  # (The pin crate's dependency graph itself is not used by the build.)
  craneLib,
  lib,
}: let
  cargoLock = fromTOML (builtins.readFile ./wasm-bindgen-cli/Cargo.lock);
  cliPkg =
    lib.findFirst (p: p.name == "wasm-bindgen-cli")
    (throw "wasm-bindgen-cli missing from nix/wasm-bindgen-cli/Cargo.lock")
    cargoLock.package;

  src = craneLib.downloadCargoPackage {
    inherit (cliPkg) name version source checksum;
  };

  # --no-default-features drops wasm-bindgen-cli's TLS/webdriver feature, which
  # the Node-based test runner doesn't need, so those crates aren't compiled.
  commonArgs = {
    inherit src;
    pname = "wasm-bindgen-cli";
    inherit (cliPkg) version;
    cargoExtraArgs = "--locked --no-default-features --bin wasm-bindgen-test-runner";
    doCheck = false;
    strictDeps = true;
  };
in
  craneLib.buildPackage (commonArgs
    // {
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
    })
