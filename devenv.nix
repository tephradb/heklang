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

  languages = {
    rust = {
      enable = true;
      channel = "nightly";
    };
  };
}
