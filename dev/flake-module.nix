{inputs, ...}: {
  imports = [
    inputs.github-actions-nix.flakeModules.default
    inputs.files.flakeModules.default
  ];

  perSystem = {
    config,
    system,
    pkgs,
    runtimeLibs,
    commonArgs,
    craneLib,
    self',
    lib,
    ...
  }: let
    hk = inputs.hk.packages.${system}.hk.overrideAttrs (_old: {
      doCheck = false;
    });
  in {
    devShells.default = craneLib.devShell {
      inherit (self') checks;

      buildInputs = runtimeLibs;
      inherit (commonArgs) nativeBuildInputs;

      packages = with pkgs; [
        # WGSL
        wgsl-analyzer

        # Git hooks
        hk
        pkl

        # Nix formatters and linters
        alejandra
        deadnix
        statix

        # TOML
        taplo

        # Markdown
        nodePackages.prettier

        # Commit messages
        gitlint
      ];

      RUST_SRC_PATH = "${pkgs.fenix.stable.rust-src}/lib/rustlib/src/rust/library";
      LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;

      shellHook = ''
        # Ensure git hooks are installed (skip in worktrees)
        if [ -d .git ]; then
          if ! output=$(hk install 2>&1); then
            exit_code=$?
            echo "$output" >&2
            exit $exit_code
          fi
        fi
      '';
    };

    # Configure files module to sync generated workflows to .github/workflows/
    files.files =
      lib.mapAttrsToList (name: drv: {
        path_ = ".github/workflows/${name}";
        inherit drv;
      })
      config.githubActions.workflowFiles;

    # Expose the files writer as an app
    apps.write-files = {
      type = "app";
      program = lib.getExe config.files.writer.drv;
      meta.description = "Write generated files to the repository";
    };

    # CI workflow configuration
    githubActions = {
      enable = true;

      workflows = {
        ci = {
          name = "CI";

          on = {
            pullRequest = {};
            workflowDispatch = {};
            push = {
              branches = ["main"];
            };
          };

          concurrency = {
            group = "\${{ github.workflow }}-\${{ github.event.pull_request.number || github.ref }}";
            cancelInProgress = true;
          };

          jobs = {
            check = {
              runsOn = "ubuntu-latest";

              permissions = {
                id-token = "write";
                contents = "read";
              };

              steps = [
                {
                  uses = "actions/checkout@v4";
                }
                {
                  uses = "DeterminateSystems/determinate-nix-action@v3";
                }
                {
                  uses = "DeterminateSystems/flakehub-cache-action@main";
                }
                {
                  name = "Run all checks";
                  run = "nix flake check";
                }
              ];
            };
          };
        };

        update-flake-lock = {
          name = "Update flake.lock";

          on = {
            workflowDispatch = {};
            schedule = [
              {cron = "0 0 * * 0";} # Weekly on Sunday at midnight
            ];
          };

          permissions = {
            id-token = "write";
            contents = "write";
            pull-requests = "write";
          };

          jobs = {
            update = {
              runsOn = "ubuntu-latest";

              steps = [
                {
                  uses = "actions/checkout@v4";
                }
                {
                  uses = "DeterminateSystems/determinate-nix-action@v3";
                }
                {
                  id = "update";
                  name = "Update flake.lock";
                  uses = "DeterminateSystems/update-flake-lock@main";
                  with_ = {
                    pr-title = "chore: update flake.lock";
                    pr-labels = "dependencies\nautomated";
                  };
                }
                {
                  name = "Enable automerge";
                  if_ = "steps.update.outputs.pull-request-number != ''";
                  run = "gh pr merge --auto --rebase \${{ steps.update.outputs.pull-request-number }}";
                  env = {
                    GH_TOKEN = "\${{ secrets.GH_TOKEN_FOR_UPDATES }}";
                  };
                }
              ];
            };
          };
        };
      };
    };
  };
}
