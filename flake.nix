{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    depot-js.url = "github:cognitive-engineering-lab/depot";
    wasm-rust = {
      type = "github";
      owner = "gavinleroy";
      repo = "rust";
      #ref = "wasm+nightly-2024-05-20";
      ref = "761bcb0ddab3ad08826bf33bc43fb50ea1652285";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, depot-js, wasm-rust }:
  flake-utils.lib.eachDefaultSystem (system:
  let 
    pkgs = import nixpkgs {
      inherit system;
    };

    # NOTE: won't it be an amazing day when we can use normal toolchains again?
    toolchain = wasm-rust.packages.${system}.default;
    depotjs = depot-js.packages.${system}.default;

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

        toolchain
      ] ++ lib.optionals stdenv.isDarwin [
        darwin.apple_sdk.frameworks.SystemConfiguration
      ];    

      shellHook = ''
        export SYSROOT=$(rustc --print sysroot)
        export MIRI_SYSROOT=$(rustc --print sysroot)
        export DYLD_LIBRARY_PATH="$DYLD_LIBRARY_PATH:$(rustc --print target-libdir)"
      '';

      RUSTC_LINKER = "${pkgs.llvmPackages.clangUseLLVM}/bin/clang";

      # NOTE: currently playwright-driver uses version 1.40.0, when something inevitably fails,
      # check that the version of playwright-driver and that of the NPM playwright
      # `packages/evaluation/package.json` match.
      PLAYWRIGHT_BROWSERS_PATH="${playwright-driver.browsers}";
    };
  });
}
