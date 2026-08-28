{
  pkgs,
  ...
}:

{
  packages = with pkgs; [ bacon ];

  languages = {
    rust = {
      enable = true;
      channel = "nightly";
    };
  };
}
