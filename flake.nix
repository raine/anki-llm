{
  description = "A command-line interface for bulk-processing Anki flashcards with LLMs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.stdenv.mkDerivation {
            pname = cargoToml.package.name;
            version = cargoToml.package.version;

            src = ./.;

            nativeBuildInputs = with pkgs; [
              cargo
              rustc
            ];

            buildPhase = ''
              runHook preBuild
              export CARGO_HOME="$TMPDIR/cargo-home"
              cargo build --release --locked
              runHook postBuild
            '';

            installPhase = ''
              runHook preInstall
              install -Dm755 target/release/anki-llm "$out/bin/anki-llm"
              runHook postInstall
            '';

            meta = with pkgs.lib; {
              description = "A command-line interface for bulk-processing Anki flashcards with LLMs";
              homepage = "https://github.com/raine/anki-llm";
              license = licenses.mit;
              mainProgram = "anki-llm";
            };
          };
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/anki-llm";
        };
      });

      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              cargo
              rustc
              rust-analyzer
              rustfmt
              clippy
            ];

            RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
          };
        }
      );
    };
}
