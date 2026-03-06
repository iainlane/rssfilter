{
  inputs,
  lib,
  flake-parts-lib,
  ...
}: {
  options.perSystem = flake-parts-lib.mkPerSystemOption (_: let
    t = lib.types;
  in {
    options.rssfilter = {
      pkgs = lib.mkOption {
        type = t.raw;
        description = "Nixpkgs instance";
      };

      rustToolchain = lib.mkOption {
        type = t.package;
        description = "Rust toolchain for building";
      };

      rustfmtNightly = lib.mkOption {
        type = t.package;
        description = "Nightly rustfmt for unstable options";
      };

      rustfmtBin = lib.mkOption {
        type = t.str;
        description = "Path to nightly rustfmt binary";
      };

      craneLib = lib.mkOption {
        type = t.raw;
        description = "Crane library configured with toolchain";
      };

      src = lib.mkOption {
        type = t.path;
        description = "Filtered source for building";
      };

      commonArgs = lib.mkOption {
        type = t.attrsOf t.raw;
        description = "Common arguments for crane builds";
      };

      wasmTarget = lib.mkOption {
        type = t.str;
        description = "WASM target triple";
      };

      wasmTestRunner = lib.mkOption {
        type = t.package;
        description = "Package containing wasm-bindgen-test-runner";
      };

      wasmTestRunnerBin = lib.mkOption {
        type = t.str;
        description = "Path to wasm-bindgen-test-runner binary";
      };

      cargoArtifactsNative = lib.mkOption {
        type = t.package;
        description = "Pre-built cargo dependencies for native checks";
      };

      cargoArtifactsWasm = lib.mkOption {
        type = t.package;
        description = "Pre-built cargo dependencies for wasm checks";
      };
    };
  });

  config.perSystem = {
    config,
    system,
    ...
  }: let
    pkgs = inputs.nixpkgs.legacyPackages.${system};
    fenixPkgs = inputs.fenix.packages.${system};
    wasmTarget = "wasm32-unknown-unknown";
    # Temporary local copy of https://github.com/NixOS/nixpkgs/pull/496279.
    # Drop this once the change lands in nixpkgs-unstable.
    wasmTestRunner = pkgs.callPackage ./wasm-bindgen-cli_0_2_114.nix {};

    rustToolchain = with fenixPkgs;
      combine [
        stable.cargo
        stable.clippy
        stable.rust-src
        stable.rust-analyzer
        stable.rustc
        targets.${wasmTarget}.stable.rust-std
      ];

    rustfmtNightly = fenixPkgs.latest.withComponents ["rustfmt" "rustc"];

    craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;
  in {
    rssfilter = {
      inherit
        pkgs
        rustToolchain
        rustfmtNightly
        craneLib
        wasmTarget
        wasmTestRunner
        ;

      rustfmtBin = lib.getExe' rustfmtNightly "rustfmt";
      wasmTestRunnerBin = lib.getExe' wasmTestRunner "wasm-bindgen-test-runner";

      src = craneLib.cleanCargoSource inputs.self.outPath;

      commonArgs = {
        inherit (config.rssfilter) src;
        strictDeps = true;
        nativeBuildInputs = [pkgs.pkg-config];
        buildInputs = [pkgs.cacert];
      };

      cargoArtifactsNative = craneLib.buildDepsOnly config.rssfilter.commonArgs;

      cargoArtifactsWasm = craneLib.buildDepsOnly (config.rssfilter.commonArgs
        // {
          cargoExtraArgs = "--locked --target ${wasmTarget}";
        });
    };
  };
}
