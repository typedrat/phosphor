{
  description = "Development inputs for phosphor. These are used by the top level flake in the dev partition, but do not appear in consumers' lock files.";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1";

    github-actions-nix = {
      url = "github:synapdeck/github-actions-nix";
    };

    files = {
      url = "github:mightyiam/files";
      flake = false;
    };
  };

  # This flake is only used for its inputs
  outputs = _: {};
}
