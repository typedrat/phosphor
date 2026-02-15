{inputs, ...}: {
  perSystem = {system, ...}: let
    pkgs = import inputs.nixpkgs {
      inherit system;
      overlays = [(import inputs.rust-overlay)];
    };

    rustToolchain = pkgs.rust-bin.stable.latest.default;
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

    commonArgs = {
      src = craneLib.cleanCargoSource inputs.self;
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
