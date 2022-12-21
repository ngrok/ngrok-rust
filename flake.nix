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
          ${toolchain}/bin/cargo clippy --fix --allow-staged --allow-dirty --all-targets
          ${toolchain}/bin/cargo fmt
        '';
        pre-commit = pkgs.writeShellScript "pre-commit" ''
          cargo clippy --all-targets -- -D warnings
          result=$?

          if [[ ''${result} -ne 0 ]] ; then
              cat <<\EOF
          There are some linting issues, try `fix-n-fmt` to fix.
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
          buildInputs = with pkgs; [
            toolchain
            fix-n-fmt
            setup-hooks
            cargo-udeps
          ];
        };
      });
}
