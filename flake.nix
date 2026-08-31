{
  description = "heklang: the module language for hekla, with its checker, test runner and formatter";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      forEachSystem = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      version = (fromTOML (builtins.readFile ./cli/Cargo.toml)).package.version;
    in
    {
      packages = forEachSystem (pkgs: rec {
        default = hek;

        # `hek check`, `hek test` and `hek fmt`. `docs/cli.md` is the contract.
        #
        # `src = self` is the whole repository as git has it, which is the point: the
        # working tree carries a multi-gigabyte `target/`, and a flake's source is the
        # tracked files alone. `cli/build.rs` compiles the committed tree-sitter parser, so
        # this needs a C compiler and not the tree-sitter CLI.
        hek = pkgs.rustPlatform.buildRustPackage {
          pname = "hek";
          inherit version;

          src = self;
          cargoLock.lockFile = ./Cargo.lock;

          # The workspace root is the library and a demo binary; `hek` is the `cli` member
          # and the only thing worth installing.
          cargoBuildFlags = [
            "-p"
            "hek"
          ];

          # The suite is `nix flake check` below, and in the repository it is a development
          # loop that runs on every change. Building the binary should not pay for it twice.
          doCheck = false;

          meta = {
            description = "Checker, test runner and formatter for heklang";
            mainProgram = "hek";
            license = pkgs.lib.licenses.mit;
            platforms = systems;
          };
        };

        # The grammar an editor loads, from the same commit as the binary. `hek fmt` links
        # its own copy of `tree-sitter-hek/src/parser.c`, so taking both from here is what
        # keeps what an editor highlights and what it formats with in step.
        #
        # ABI 14 is baked into the committed `src/`, which is what
        # `tree-sitter-hek/README.md` requires and what helix loads.
        tree-sitter-hek = pkgs.tree-sitter.buildGrammar {
          language = "hek";
          version = self.shortRev or "dirty";
          src = ./tree-sitter-hek;
        };
      });

      # `nix flake check` runs the whole suite, which the package deliberately does not.
      checks = forEachSystem (pkgs: {
        tests = self.packages.${pkgs.system}.hek.overrideAttrs (_: {
          pname = "hek-tests";
          doCheck = true;
          cargoTestFlags = [ "--workspace" ];
        });
      });
    };
}
