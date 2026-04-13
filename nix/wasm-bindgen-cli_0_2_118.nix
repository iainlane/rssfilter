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
    hash = "sha256-D8+jijPlrD32sRQAXp9oJhYXww2IKtJxJQklxuJy02k=";
  };

  cargoDeps = rustPlatform.fetchCargoVendor {
    inherit src;
    inherit (src) pname version;
    hash = "sha256-mUQKo4ijP7N06i0DGhAa5+J13OyO4sW7eCNwOjzl4d4=";
  };
}
