{
  description = "FreeFlow — gestion pour indépendant (backend Rust, coque Tauri, CLI, MCP)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";

    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        inherit (pkgs) lib stdenv;

        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        craneLib = (crane.mkLib pkgs).overrideToolchain (_: rustToolchain);

        # Tauri v2 (lot 9) a besoin du toolkit GTK/WebKit pour rendre sa fenêtre native sur Linux.
        linuxNativeDeps = with pkgs; lib.optionals stdenv.hostPlatform.isLinux [
          webkitgtk_4_1
          libsoup_3
          gtk3
          glib-networking
          librsvg
        ];

        # Sur macOS, la webview native s'appuie sur WebKit.framework fourni par le SDK Apple.
        darwinNativeDeps = lib.optionals stdenv.hostPlatform.isDarwin
          (with pkgs.darwin.apple_sdk.frameworks; [
            WebKit
            AppKit
            Security
            CoreServices
          ]);

        # `cleanCargoSource` ne garde que les fichiers "Rust-shaped" et élimine sinon les
        # assets référencés via `include_str!`/`include_bytes!` (migrations SQL, CSS/JS
        # vendorisés du lot 9, templates Typst à venir en lot 10, etc.), ainsi que les fichiers
        # de configuration Tauri lus directement par `tauri-build` au moment de la compilation
        # (`tauri.conf.json`, `capabilities/*.json`, l'icône, le `dist/index.html` placeholder)
        # : on complète le filtre plutôt que de perdre le cache incrémental d'un `src = ./.`
        # non filtré.
        nonRustAssets =
          path: _type: builtins.match ".*\\.(sql|typst|css|js|json|png|html)$" path != null;
        src = lib.cleanSourceWith {
          src = craneLib.path ./.;
          filter = path: type: (craneLib.filterCargoSources path type) || (nonRustAssets path type);
        };

        commonArgs = {
          pname = "freeflow";
          version = "0.1.0";
          inherit src;
          strictDeps = true;

          buildInputs = [
            pkgs.sqlcipher
            pkgs.openssl
          ] ++ linuxNativeDeps ++ darwinNativeDeps;

          nativeBuildInputs = [ pkgs.pkg-config ];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        freeflow = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          doCheck = false; # les tests tournent via `just test` (cargo-nextest), pas au build Nix.
        });
      in
      {
        packages.default = freeflow;

        checks = {
          inherit freeflow;

          freeflow-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--workspace --all-targets -- -D warnings";
          });

          freeflow-fmt = craneLib.cargoFmt {
            pname = "freeflow";
            version = "0.1.0";
            inherit src;
          };
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ freeflow ];

          packages = with pkgs; [
            rustToolchain
            cargo-nextest
            cargo-deny
            cargo-audit
            cargo-insta
            cargo-machete
            just
            typst
            libxml2 # `xmllint` : validation XSD du XML CII contre le schéma EN 16931 (lot 10)
            sqlcipher
            pkg-config
            openssl
          ] ++ linuxNativeDeps ++ darwinNativeDeps;

          shellHook = ''
            echo "freeflow · $(rustc --version)"
          '';

          RUST_BACKTRACE = "1";
        };
      }
    );
}
