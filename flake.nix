{
  description = "A tiny AI Agent";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  inputs.crane.url = "github:ipetkov/crane";

  outputs =
    {
      self,
      nixpkgs,
      crane,
      ...
    }:
    let
      # Commands run only in the Linux sandbox; there is no macOS backend.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      # The pinned nixpkgs contains 0.11.0. Require the upstream fix for
      # CVE-2026-87766 without updating unrelated toolchain dependencies.
      sandboxBackend =
        pkgs:
        pkgs.bubblewrap.overrideAttrs (old: rec {
          version = "0.12.0";
          src = pkgs.fetchurl {
            url = "https://github.com/containers/bubblewrap/releases/download/v${version}/bubblewrap-${version}.tar.xz";
            hash = "sha256-l2DQBzY+Orunx0dImRD5+C2fylO6O9MoLjlvo8l6MxQ=";
          };
          meta = old.meta // {
            changelog = "https://github.com/containers/bubblewrap/releases/tag/v${version}";
            license = pkgs.lib.licenses.lgpl21Plus;
          };
        });
      # The pinned nixpkgs contains 0.6.2, which ignores `no_edit` and drops
      # non-ASCII titles from ADR file names. Build the release CI checks with.
      adrs =
        pkgs:
        pkgs.rustPlatform.buildRustPackage (finalAttrs: {
          pname = "adrs";
          version = "0.13.0";
          src = pkgs.fetchFromGitHub {
            owner = "joshrotenberg";
            repo = "adrs";
            tag = "v${finalAttrs.version}";
            hash = "sha256-XBSGK63MEpCy2jNqn2XPUmKXgmmRAYFKPupoRvG5R+8=";
          };
          cargoHash = "sha256-sdkUYSC2OmVIL8b3u6q0Lv9yWRiijTzI7l7ch3tszsc=";
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.oniguruma ];
          meta = {
            description = "Command-line tool for managing Architectural Decision Records";
            homepage = "https://github.com/joshrotenberg/adrs";
            license = with pkgs.lib.licenses; [
              mit
              asl20
            ];
            mainProgram = "adrs";
          };
        });
    in
    {
      formatter = forAllSystems (pkgs: pkgs.nixfmt);

      packages = forAllSystems (
        pkgs:
        let
          lib = pkgs.lib;
          craneLib = crane.mkLib pkgs;
          # Only what the crates build and test with, so that changes to
          # docs or CI do not rebuild the package.
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./crates
              # Example files that tests read with include_str!, such as
              # models.toml.example, config.toml.example and agent.md.example.
              (lib.fileset.fileFilter (file: lib.hasSuffix ".example" file.name) ./.)
            ];
          };
          commonArgs = {
            inherit src;
            pname = "socium";
            version = "0.1.0";
            strictDeps = true;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.openssl ];
            SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
            NIX_SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
          };
          # Dependencies alone, built from Cargo.toml and Cargo.lock. Its
          # output stays the same until they change, so the Cachix cache
          # serves it to later builds.
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        in
        {
          default = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.makeWrapper ];
              nativeCheckInputs = [
                pkgs.git
                pkgs.ripgrep
              ]
              ++ lib.optionals pkgs.stdenv.isLinux [
                (sandboxBackend pkgs)
                pkgs.util-linux # setsid for detached-descendant enforcement tests
                # Toolchains the sandbox acceptance test runs.
                pkgs.python3
                pkgs.nodejs
              ];

              postFixup = ''
                wrapProgram "$out/bin/socium" \
                  --prefix PATH : "${
                    lib.makeBinPath (
                      [
                        pkgs.git
                        pkgs.ripgrep
                      ]
                      ++ lib.optionals pkgs.stdenv.isLinux [ (sandboxBackend pkgs) ]
                    )
                  }" \
                  --set-default SSL_CERT_FILE "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt" \
                  --set-default NIX_SSL_CERT_FILE "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              '';
            }
          );
          deps = cargoArtifacts;
          adrs = adrs pkgs;
        }
      );

      apps = forAllSystems (pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.system}.default}/bin/socium";
        };
      });

      devShells = forAllSystems (
        pkgs:
        let
          lib = pkgs.lib;
        in
        {
          default = pkgs.mkShell {
            packages = [
              pkgs.rustc
              pkgs.cargo
              pkgs.clippy
              pkgs.rustfmt
              pkgs.rust-analyzer
              pkgs.git
              pkgs.ripgrep
              pkgs.cacert
              pkgs.openssl
              pkgs.pkg-config
              pkgs.nixfmt
              pkgs.pre-commit
              (adrs pkgs)
            ]
            ++ lib.optionals pkgs.stdenv.isLinux [
              (sandboxBackend pkgs)
              pkgs.util-linux
              pkgs.python3
              pkgs.nodejs
            ];
          };
        }
      );
    };
}
