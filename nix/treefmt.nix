{inputs, ...}: {
  imports = [
    inputs.treefmt-nix.flakeModule
  ];

  perSystem = {config, ...}: let
    inherit (config.rssfilter) pkgs rustfmtNightly;
    inherit (pkgs) lib;

    statixBinary = lib.getExe pkgs.statix;
    markdownlintBinary = lib.getExe pkgs.markdownlint-cli;
  in {
    treefmt.config = {
      programs = {
        actionlint.enable = true;
        alejandra.enable = true;
        deadnix.enable = true;
        rustfmt = {
          enable = true;
          package = rustfmtNightly;
        };
        # We use `markdownlint-cli` instead of `mdformat`.
        mdformat.enable = false;
        nixf-diagnose = {
          enable = true;
          variableLookup = true;
        };
        statix.enable = true;
        stylua.enable = true;
        prettier = {
          enable = true;
          settings = {
            proseWrap = "always";
          };
        };
        zizmor.enable = true;
      };

      projectRootFile = "flake.nix";

      settings = {
        excludes = [
          "**/lazy-lock.json"
          "pnpm-lock.yaml"
          "**/pnpm-lock.yaml"
        ];

        # markdownlint-cli exits non-zero for unfixable violations,
        # which treefmt treats as a formatter failure. Wrap it so
        # treefmt always picks up the fixes it did apply.
        formatter.markdownlint = {
          command = lib.getExe (pkgs.writeShellScriptBin "markdownlint-fix" ''
            ${lib.getExe pkgs.markdownlint-cli} --fix "$@" || true
          '');
          includes = ["*.md"];
        };
      };
    };

    checks.statix =
      pkgs.runCommandLocal "statix-check" {}
      ''
        set -e

        cd ${lib.escapeShellArg inputs.self}
        ${statixBinary} check .

        touch $out
      '';

    checks.markdownlint =
      pkgs.runCommandLocal "markdownlint-check" {}
      ''
        set -e

        cd ${lib.escapeShellArg inputs.self}
        ${markdownlintBinary} "**/*.md"

        touch $out
      '';
  };
}
