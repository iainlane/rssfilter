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

    buildWasm = craneLib.buildPackage (commonArgs
      // {
        cargoArtifacts = cargoArtifactsWasm;
        CARGO_BUILD_TARGET = wasmTarget;
        cargoExtraArgs = "--locked --target ${wasmTarget} -p workers-rssfilter";
        doCheck = false;
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
    };
  };
}
