{inputs, ...}: {
  perSystem = {system, ...}: let
    pkgs = import inputs.nixpkgs {
      inherit system;
      overlays = [inputs.fenix.overlays.default];
    };

    toolchain = pkgs.fenix.stable;
    rustToolchain = toolchain.withComponents [
      "cargo"
      "clippy"
      "rustc"
      "rustfmt"
      "rust-analyzer"
      "rust-src"
    ];
    craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;

    runtimeLibs = with pkgs; [
      vulkan-loader
      wayland
      libxkbcommon
      libGL
      libx11
      libxcursor
      libxi
      libxrandr
    ];

    src = pkgs.lib.cleanSourceWith {
      src = inputs.self;
      filter = path: type:
        (craneLib.filterCargoSources path type)
        || (builtins.match ".*\\.(wgsl|csv|toml)$" path != null);
    };

    commonArgs = {
      inherit src;
      strictDeps = true;

      buildInputs = runtimeLibs;
      nativeBuildInputs = with pkgs; [pkg-config];
    };

    cargoArtifacts = craneLib.buildDepsOnly commonArgs;

    phosphor = craneLib.buildPackage (commonArgs
      // {
        inherit cargoArtifacts;

        nativeBuildInputs = commonArgs.nativeBuildInputs ++ [pkgs.makeWrapper];

        postInstall = ''
          wrapProgram $out/bin/phosphor \
            --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath runtimeLibs}
        '';
      });
  in {
    _module.args = {
      inherit pkgs craneLib commonArgs cargoArtifacts runtimeLibs;
    };

    packages.default = phosphor;
  };
}
