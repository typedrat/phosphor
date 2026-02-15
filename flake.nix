{
  description = "Phosphor — physically-based X-Y CRT simulator";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1";
    flake-parts.url = "https://flakehub.com/f/hercules-ci/flake-parts/0.1";
    crane.url = "https://flakehub.com/f/ipetkov/crane/*";
    fenix = {
      url = "https://flakehub.com/f/nix-community/fenix/0.1";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs:
    inputs.flake-parts.lib.mkFlake {inherit inputs;} {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      imports = [
        inputs.flake-parts.flakeModules.partitions
        ./flake/packages.nix
        ./flake/checks.nix
      ];

      partitionedAttrs = {
        devShells = "dev";
        checks = "dev";
        apps = "dev";
      };

      partitions.dev = {
        extraInputsFlake = ./dev;
        module.imports = [
          ./dev/flake-module.nix
        ];
      };
    };
}
