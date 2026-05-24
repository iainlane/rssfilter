{inputs, ...}: {
  perSystem = {config, ...}: let
    inherit
      (config.rssfilter)
      pkgs
      craneLib
      commonArgs
      cargoArtifactsNative
      cargoArtifactsWasm
      wasmTarget
      wasmTestRunner
      wasmTestRunnerBin
      ;

    inherit (pkgs) lib;

    buildWasm = craneLib.buildPackage (commonArgs
      // {
        cargoArtifacts = cargoArtifactsWasm;
        CARGO_BUILD_TARGET = wasmTarget;
        cargoExtraArgs = "--locked --target ${wasmTarget} -p workers-rssfilter";
        doCheck = false;
      });

    # The Trunk build needs the frontend's `index.html`, CSS and `Trunk.toml`
    # alongside the Cargo sources, which `cleanCargoSource` strips.
    frontendSrc = lib.cleanSourceWith {
      src = inputs.self.outPath;
      name = "frontend-source";
      filter = path: type:
        (lib.hasSuffix ".html" path)
        || (lib.hasSuffix ".css" path)
        || (baseNameOf path == "Trunk.toml")
        || (craneLib.filterCargoSources path type);
    };

    frontendArgs =
      commonArgs
      // {
        src = frontendSrc;
        pname = "frontend";
        CARGO_BUILD_TARGET = wasmTarget;
        cargoExtraArgs = "--locked -p frontend";
      };

    frontendDeps = craneLib.buildDepsOnly frontendArgs;

    # The single-page app bundle (wasm + JS loader + HTML + CSS) served as
    # static assets. Pin wasm-bindgen-cli to the workspace's version.
    buildFrontend = craneLib.buildTrunkPackage (frontendArgs
      // {
        cargoArtifacts = frontendDeps;
        trunkIndexPath = "frontend/index.html";
        wasm-bindgen-cli = wasmTestRunner;
      });
  in {
    checks = {
      audit = craneLib.cargoAudit (commonArgs
        // {
          advisory-db = inputs."advisory-db";
        });

      clippy = craneLib.cargoClippy (commonArgs
        // {
          cargoArtifacts = cargoArtifactsNative;
          cargoClippyExtraArgs = "--all-targets -- --deny warnings";
        });

      tests = craneLib.cargoTest (commonArgs
        // {
          cargoArtifacts = cargoArtifactsNative;
        });

      tests-wasm = craneLib.cargoTest (commonArgs
        // {
          cargoArtifacts = cargoArtifactsWasm;
          CARGO_BUILD_TARGET = wasmTarget;
          cargoExtraArgs = "--locked --target ${wasmTarget}";
          nativeBuildInputs = commonArgs.nativeBuildInputs ++ [wasmTestRunner pkgs.nodejs];
          preCheck = ''
            export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=${wasmTestRunnerBin}
          '';
        });

      doc = craneLib.cargoDoc (commonArgs
        // {
          cargoArtifacts = cargoArtifactsNative;
          env.RUSTDOCFLAGS = "--deny warnings";
        });

      build = config.packages.rssfilter;
      build-wasm = buildWasm;
      build-frontend = buildFrontend;
    };
  };
}
