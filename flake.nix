{
  description = "ngrok agent library in Rust";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";

    # Note: fenix packages are cached via cachix:
    #       cachix use nix-community
    fenix-flake = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils = {
      url = "github:numtide/flake-utils";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, fenix-flake, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            fenix-flake.overlays.default
          ];
        };
        toolchain = pkgs.fenix.complete.withComponents [
          "cargo"
          "clippy"
          "rust-src"
          "rustc"
          "rustfmt"
        ];
        fix-n-fmt = pkgs.writeShellScriptBin "fix-n-fmt" ''
          set -euf -o pipefail
          ${toolchain}/bin/cargo clippy --fix --allow-staged --allow-dirty --all-targets --all-features
          ${toolchain}/bin/cargo fmt
        '';
        pre-commit = pkgs.writeShellScript "pre-commit" ''
          cargo clippy --workspace --all-targets --all-features -- -D warnings
          result=$?

          if [[ ''${result} -ne 0 ]] ; then
              cat <<\EOF
          There are some linting issues, try `fix-n-fmt` to fix.
          EOF
              exit 1
          fi

          # Use a dedicated sub-target-dir for udeps. For some reason, it fights with clippy over the cache.
          CARGO_TARGET_DIR=$(git rev-parse --show-toplevel)/target/udeps cargo udeps --workspace --all-targets --all-features
          result=$?

          if [[ ''${result} -ne 0 ]] ; then
              cat <<\EOF
          There are some unused dependencies.
          EOF
              exit 1
          fi

          diff=$(cargo fmt -- --check)
          result=$?

          if [[ ''${result} -ne 0 ]] ; then
              cat <<\EOF
          There are some code style issues, run `fix-n-fmt` first.
          EOF
              exit 1
          fi

          exit 0
        '';
        setup-hooks = pkgs.writeShellScriptBin "setup-hooks" ''
          repo_root=$(git rev-parse --git-dir)

          ${toString (map (h: ''
            ln -sf ${h} ''${repo_root}/hooks/${h.name}
          '') [
            pre-commit
          ])}
        '';
      in
      {
        devShell = pkgs.mkShell {
          CHALK_OVERFLOW_DEPTH = 3000;
          CHALK_SOLVER_MAX_SIZE = 1500;
          OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
          OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";
          buildInputs = with pkgs; [
            toolchain
            fix-n-fmt
            setup-hooks
            cargo-udeps
          ] ++ lib.optionals stdenv.isDarwin [
            # nix darwin stdenv has broken libiconv: https://github.com/NixOS/nixpkgs/issues/158331
            libiconv
            pkgs.darwin.apple_sdk.frameworks.Security
          ];
        };
      });
}
