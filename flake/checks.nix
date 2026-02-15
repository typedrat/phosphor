{
  perSystem = {
    craneLib,
    commonArgs,
    cargoArtifacts,
    self',
    ...
  }: {
    checks = {
      inherit (self'.packages) default;

      clippy = craneLib.cargoClippy (commonArgs
        // {
          inherit cargoArtifacts;
          cargoClippyExtraArgs = "--all-targets -- --deny warnings";
        });
    };
  };
}
