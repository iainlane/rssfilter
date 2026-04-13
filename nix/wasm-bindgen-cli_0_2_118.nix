{
  # Temporary local copy of https://github.com/NixOS/nixpkgs/pull/496279.
  # Drop this once the change lands in nixpkgs-unstable.
  buildWasmBindgenCli,
  fetchCrate,
  rustPlatform,
}:
buildWasmBindgenCli rec {
  src = fetchCrate {
    pname = "wasm-bindgen-cli";
    version = "0.2.118";
    hash = "sha256-97V+u/u5Uqt539dzm/fOPOoJiDKFKkabGAr5iDAW2dg=";
  };

  cargoDeps = rustPlatform.fetchCargoVendor {
    inherit src;
    inherit (src) pname version;
    hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
  };
}
