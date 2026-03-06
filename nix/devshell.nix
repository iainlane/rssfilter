_: {
  perSystem = {config, ...}: let
    inherit
      (config.rssfilter)
      pkgs
      rustToolchain
      rustfmtNightly
      rustfmtBin
      wasmTestRunner
      ;
  in {
    devShells.default = pkgs.mkShell {
      buildInputs = [
        rustToolchain
        rustfmtNightly
        wasmTestRunner
        pkgs.nodejs
        pkgs.pkg-config
        pkgs.cargo-llvm-cov
        pkgs.just
      ];
      RUSTFMT = rustfmtBin;
    };
  };
}
