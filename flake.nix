{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs = {
    self,
    nixpkgs,
    fenix,
    crane,
    ...
  }: let
    systems = ["x86_64-linux" "aarch64-darwin"];
    forAllSystems = f:
      nixpkgs.lib.genAttrs systems (system:
        f {
          pkgs = nixpkgs.legacyPackages.${system};
          fenixPkgs = fenix.packages.${system};
          craneLib =
            (crane.mkLib nixpkgs.legacyPackages.${system}).overrideToolchain
            fenix.packages.${system}.stable.toolchain;
        });

    perSystem = forAllSystems ({
      pkgs,
      fenixPkgs,
      craneLib,
    }: let
      src = craneLib.cleanCargoSource ./.;
      onnxruntime-bin = pkgs.callPackage ./nix/onnxruntime.nix {};

      commonArgs =
        {
          inherit src;
          pname = "meridian";
          strictDeps = true;
          nativeBuildInputs =
            [
              pkgs.pkg-config
              pkgs.llvmPackages.clang
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
              pkgs.openssl
            ];
          buildInputs =
            pkgs.lib.optionals pkgs.stdenv.isLinux [
              pkgs.openssl
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              pkgs.libiconv
              pkgs.apple-sdk_26
            ];
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          ORT_DYLIB_PATH = "${onnxruntime-bin}/lib/libonnxruntime${pkgs.stdenv.hostPlatform.extensions.sharedLibrary}";
          ORT_LIB_LOCATION = "${onnxruntime-bin}/lib";
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [onnxruntime-bin pkgs.openssl];
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.isDarwin {
          DYLD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [onnxruntime-bin];
        };

      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      meridianUnwrapped = craneLib.buildPackage (commonArgs
        // {
          inherit cargoArtifacts;
        });

      meridian = pkgs.symlinkJoin {
        name = "meridian";
        paths = [meridianUnwrapped];
        nativeBuildInputs = [pkgs.makeWrapper];
        postBuild = ''
          wrapProgram $out/bin/meridian \
            --set ORT_DYLIB_PATH "${onnxruntime-bin}/lib/libonnxruntime${pkgs.stdenv.hostPlatform.extensions.sharedLibrary}"
        '';
      };

      toolchain = fenixPkgs.stable.withComponents [
        "cargo"
        "clippy"
        "rustc"
        "rustfmt"
        "rust-src"
        "rust-analyzer"
        "llvm-tools"
      ];
    in {
      packages = {
        inherit meridian cargoArtifacts;
        default = meridian;
      };

      checks = {
        fmt = craneLib.cargoFmt {
          inherit src;
          pname = "meridian";
        };

        taplo =
          pkgs.runCommand "taplo-check" {
            nativeBuildInputs = [pkgs.taplo];
          } ''
            cd ${self}
            taplo check
            touch $out
          '';

        typos =
          pkgs.runCommand "typos-check" {
            nativeBuildInputs = [pkgs.typos];
          } ''
            cd ${self}
            typos
            touch $out
          '';

        nix-fmt =
          pkgs.runCommand "nix-fmt-check" {
            nativeBuildInputs = [pkgs.alejandra];
          } ''
            alejandra --check ${self}/flake.nix ${self}/nix/
            touch $out
          '';
      };

      devShells.default = pkgs.mkShell {
        packages =
          [
            toolchain
            pkgs.cargo-nextest
            pkgs.cargo-llvm-cov
            pkgs.cargo-deny
            pkgs.taplo
            pkgs.typos
            pkgs.llvmPackages.clang
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.pkg-config
            pkgs.openssl
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.apple-sdk_26
          ];

        env =
          {
            RUST_BACKTRACE = "1";
            RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
            ORT_DYLIB_PATH = "${onnxruntime-bin}/lib/libonnxruntime${pkgs.stdenv.hostPlatform.extensions.sharedLibrary}";
            ORT_LIB_LOCATION = "${onnxruntime-bin}/lib";
          }
          // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [pkgs.openssl pkgs.stdenv.cc.cc.lib onnxruntime-bin];
          };
      };
    });
  in {
    formatter = nixpkgs.lib.genAttrs systems (system: nixpkgs.legacyPackages.${system}.alejandra);
    packages = nixpkgs.lib.mapAttrs (_: s: s.packages) perSystem;
    checks = nixpkgs.lib.mapAttrs (_: s: s.checks) perSystem;
    devShells = nixpkgs.lib.mapAttrs (_: s: s.devShells) perSystem;
  };
}
