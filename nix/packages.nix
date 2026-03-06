_: {
  perSystem = {config, ...}: let
    inherit
      (config.rssfilter)
      craneLib
      commonArgs
      cargoArtifactsNative
      ;

    rssfilter = craneLib.buildPackage (commonArgs
      // {
        cargoArtifacts = cargoArtifactsNative;
        cargoExtraArgs = "--locked -p rssfilter-cli";
        meta.mainProgram = "rssfilter";
      });
  in {
    packages = {
      inherit rssfilter;
      default = rssfilter;
    };
  };
}
