{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    depot-js.url = "github:cognitive-engineering-lab/depot";
    wasm-rust = {
      type = "github";
      owner = "gavinleroy";
      repo = "rust";
      ref = "wasm+nightly-2024-05-20";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, depot-js, wasm-rust }:
  flake-utils.lib.eachSystem [
    "x86_64-linux"
    "aarch64-darwin"
  ] (system:
  let 
    overlays = [ (import rust-overlay) ];
    pkgs = import nixpkgs {
      inherit system overlays;
    };

    # NOTE: won't it be an amazing day when we can use normal toolchains again?
    toolchain = wasm-rust.packages.${system}.default;
    # toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
    depotjs = depot-js.packages.${system}.default;

    rustc-host = if system == "x86_64-linux"
                  then "x86_64-unknown-linux-gnu"
                 else
                   if system == "aarch64-darwin"
                  then "aarch64-apple-darwin"
                  else throw "unsupported system ${system}";

    ci-check = pkgs.writeScriptBin "ci-check" ''
      cargo insta test
      cd crates/mdbook-aquascope/test-book && mdbook build
      cd frontend && depot test && cd ..
    '';

    ci-build-pages = pkgs.writeScriptBin "ci-update-frontend" ''
      cargo doc --lib
      mv ./target/doc ./frontend/packages/aquascope-standalone/dist/doc
      cd frontend && depot build
    '';

    ci-build = pkgs.writeScriptBin "ci-build" ''
      cargo build --release -p mdbook-aquascope -p aquascope_front
    '';

    ci-install = pkgs.writeScriptBin "ci-install" ''
      cargo install --path crates/aquascope_front --debug --locked
      cargo install --path crates/mdbook-aquascope --debug --locked
    '';

    ci-publish-crates = pkgs.writeScriptBin "ci-publish-crates" ''
      cargo build
      cargo ws publish --from-git --allow-dirty --yes --token "$1"
    '';

  in {
    devShell = with pkgs; mkShell {
      buildInputs = [
        ci-check
        ci-install
        ci-build
        ci-build-pages
        ci-publish-crates

        llvmPackages_latest.llvm
        llvmPackages_latest.lld
        libiconv
        
        depotjs
        nodejs_22
        nodePackages.pnpm


        cargo-insta
        cargo-make
        cargo-watch
        rust-analyzer

        mdbook

        rustup
        toolchain
      ] ++ lib.optionals stdenv.isDarwin [
        darwin.apple_sdk.frameworks.SystemConfiguration
      ];    

      # HACK: whoooooof, this is bad.
      shellHook = ''
        rustup toolchain link wasm-nightly-2024-05-20 "${toolchain}/${rustc-host}/stage1"
        export DYLD_LIBRARY_PATH="$DYLD_LIBRARY_PATH:${toolchain}/${rustc-host}/stage1/lib"
        export PATH="$PATH:${toolchain}/${rustc-host}/stage0/lib/rustlib/${rustc-host}/bin"
      '';

      RUSTC_LINKER = "${pkgs.llvmPackages.clangUseLLVM}/bin/clang";

      # NOTE: currently playwright-driver uses version 1.40.0, when something inevitably fails,
      # check that the version of playwright-driver and that of the NPM playwright
      # `packages/evaluation/package.json` match.
      PLAYWRIGHT_BROWSERS_PATH="${playwright-driver.browsers}";
    };
  });
}
