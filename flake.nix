{
  description = "A tiny AI Agent";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
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
    in
    {
      formatter = forAllSystems (pkgs: pkgs.nixfmt);

      packages = forAllSystems (
        pkgs:
        let
          lib = pkgs.lib;
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "socium";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.makeWrapper
            ];
            nativeCheckInputs = [
              pkgs.git
              pkgs.ripgrep
            ]
            ++ lib.optionals pkgs.stdenv.isLinux [
              (sandboxBackend pkgs)
              pkgs.util-linux # setsid for detached-descendant enforcement tests
            ];
            buildInputs = [
              pkgs.openssl
            ]
            ++ lib.optionals pkgs.stdenv.isDarwin [
              pkgs.darwin.apple_sdk.frameworks.Security
              pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
            ];
            SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
            NIX_SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";

            postFixup = ''
              wrapProgram "$out/bin/socium-tui" \
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
          };
        }
      );

      apps = forAllSystems (pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.system}.default}/bin/socium-tui";
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
            ]
            ++ lib.optionals pkgs.stdenv.isLinux [
              (sandboxBackend pkgs)
              pkgs.util-linux
            ]
            ++ lib.optionals pkgs.stdenv.isDarwin [
              pkgs.darwin.apple_sdk.frameworks.Security
              pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
            ];
          };
        }
      );
    };
}
