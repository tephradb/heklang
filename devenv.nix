{
  pkgs,
  ...
}:

{
  # `tree-sitter` and `nodejs` are for tree-sitter-hek; `generate` evaluates grammar.js
  # with node.
  packages = with pkgs; [
    bacon
    nodejs
    tree-sitter
  ];

  # A bare `generate` picks the newest ABI, and the committed parser is pinned to the one
  # Helix loads. CI regenerates and diffs, so the default would fail there rather than here.
  env.TREE_SITTER_ABI_VERSION = "14";

  languages = {
    rust = {
      enable = true;
      channel = "nightly";
    };
  };
}
