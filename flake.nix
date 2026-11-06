{
  description = "mathfmt: a filter that enforces math-notation conventions in LaTeX source";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      lib = nixpkgs.lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;

      # What a build sees: the manifests, the lockfile, the crates (the data
      # tables are embedded from crates/mathfmt-core/data) and tests/ (the
      # golden fixtures). Not target/, docs/ or the rig.
      src = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./crates
          ./tests
        ];
      };

      mkPackage =
        pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = "mathfmt";
          inherit version src;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [
            "--package"
            "mathfmt"
          ];
          cargoTestFlags = [ "--workspace" ];
          doCheck = true;
          meta = {
            description = "A filter that enforces math-notation conventions in LaTeX source";
            mainProgram = "mathfmt";
            platforms = systems;
          };
        };
    in
    {
      packages = forAllSystems (pkgs: rec {
        mathfmt = mkPackage pkgs;
        default = mathfmt;
      });

      devShells = forAllSystems (
        pkgs:
        let
          # Every crate Cargo.lock names, unpacked with its checksum file, as
          # cargo's vendored-source layout. tests/dev.sh points cargo at it, so
          # a phalanix seat (whose proxy allows no crates.io) and a laptop with
          # no network both build; the nix daemon fetches the crates instead.
          vendor = pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              rustc
              clippy
              rustfmt
              rust-analyzer
              python3 # tools/gen-table.py
            ];
            CARGO_VENDOR_DIR = "${vendor}";
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          };
        }
      );

      checks = forAllSystems (
        pkgs:
        let
          mathfmt = mkPackage pkgs;
        in
        {
          # The package, with `cargo test --workspace` as its check phase.
          build = mathfmt;

          # The same vendored build, running clippy instead of a build.
          clippy = mathfmt.overrideAttrs (old: {
            pname = "mathfmt-clippy";
            nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.clippy ];
            buildPhase = ''
              runHook preBuild
              cargo clippy --workspace --all-targets --offline -- -D warnings
              runHook postBuild
            '';
            doCheck = false;
            installPhase = "touch $out";
          });

          fmt =
            pkgs.runCommand "mathfmt-fmt"
              {
                nativeBuildInputs = [
                  pkgs.cargo
                  pkgs.rustfmt
                ];
              }
              ''
                cd ${src}
                export CARGO_HOME=$TMPDIR/cargo
                cargo fmt --all --check
                touch $out
              '';
        }
      );
    };
}
