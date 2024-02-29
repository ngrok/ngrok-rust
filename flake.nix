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
        # Make sure that cargo semver-checks uses the stable toolchain rather
        # than the nightly one that we normally develop with.
        semver-checks = with pkgs; symlinkJoin {
          name = "cargo-semver-checks";
          paths = [ cargo-semver-checks ];
          buildInputs = [ makeWrapper ];
          postBuild = ''
            wrapProgram $out/bin/cargo-semver-checks \
              --prefix PATH : ${rustc}/bin \
              --prefix PATH : ${cargo}/bin
          '';
        };
        extract-version = with pkgs; writeShellScriptBin "extract-crate-version" ''
          ${cargo}/bin/cargo metadata --format-version 1 --no-deps | \
            ${jq}/bin/jq -r ".packages[] | select(.name == \"$1\") | .version"
        '';
        lib = pkgs.lib;
        libclang-path = with pkgs; symlinkJoin {
          name = "libclang";
          paths = [
            libclang.dev
            libclang.lib
          ];
        };
        rustPlatform = pkgs.makeRustPlatform {
          cargo = toolchain;
          rustc = toolchain;
        };
        cargoWorkspace = pkgs.callPackage ./Cargo.nix {
          buildRustCrateForPkgs = pkgs: pkgs.buildRustCrate.override {
            rustc = toolchain;
            defaultCrateOverrides = pkgs.defaultCrateOverrides // {
              ngrok-nginx = attrs: {
                postInstall = ''
                  ${if builtins.hasAttr "postInstall" attrs then attrs.postInstall else ""}
                  ln -s $lib/lib/libngrok_nginx-*.a $lib/lib/libngrok_nginx.a
                '';
              };
            };
          };
        };
        ngrok-nginx = cargoWorkspace.workspaceMembers.ngrok-nginx.build.lib;
        nginx-ngrok-module = {
          name = "ngrok";
          src = ./ngrok-nginx/nginx-module;
          inputs = [ ngrok-nginx ];
          meta = {
            license = with lib.licenses; [ mit asl20 ];
          };
        };
      in
      {
        packages.ngrok-nginx = cargoWorkspace.workspaceMembers.ngrok-nginx.build.lib;
        packages.nginx = pkgs.enableDebugging (pkgs.nginx.override {
          dontStrip = true;
          modules = [
            nginx-ngrok-module
          ];
        });
        devShell = with pkgs;
          mkShell {
            CHALK_OVERFLOW_DEPTH = 3000;
            CHALK_SOLVER_MAX_SIZE = 1500;
            OPENSSL_LIB_DIR = "${openssl.out}/lib";
            OPENSSL_INCLUDE_DIR = "${openssl.dev}/include";
            LIBCLANG_PATH = "${libclang-path}/lib";
            # BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${clang}/resource-root/include -isystem ${glibc.dev}/include $NIX_CFLAGS_COMPILE -isystem ${libxcrypt}/include -isystem ${pcre.dev}/include";
            BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${clang}/resource-root/include -isystem ${glibc.dev}/include";
            RUSTC_WRAPPER = "${sccache}/bin/sccache";
            shellHook = ''
              export BINDGEN_EXTRA_CLANG_ARGS="$BINDGEN_EXTRA_CLANG_ARGS $NIX_CFLAGS_COMPILE"
            '';
            inputsFrom = [
              ngrok-nginx
              # Use the base nginx package for the devshell rather than our
              # flavor that also pulls in the actual build of the rust library.
              pkgs.nginx
            ];
            buildInputs = with pkgs; [
              crate2nix
              toolchain
              fix-n-fmt
              setup-hooks
              cargo-udeps
              semver-checks
              extract-version
              pcre
              openssl
              zlib
              libxcrypt
              gdb
            ] ++ lib.optionals stdenv.isDarwin [
              # nix darwin stdenv has broken libiconv: https://github.com/NixOS/nixpkgs/issues/158331
              libiconv
              darwin.apple_sdk.frameworks.CoreServices
              darwin.apple_sdk.frameworks.Security
            ];
          };
      });
}
