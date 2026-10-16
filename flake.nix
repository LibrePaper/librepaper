# Repackages the release .deb from GitHub Releases; no source build.
# nix/source.json pins the release that this flake installs.
{
  description = "LibrePaper desktop app and command line, from the release .deb";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      librepaper = pkgs.callPackage ./nix/package.nix { };
    in
    {
      packages.${system} = {
        default = librepaper;
        inherit librepaper;
      };

      apps.${system} = {
        default = {
          type = "app";
          program = pkgs.lib.getExe' librepaper "librepaper-desktop";
        };
        cli = {
          type = "app";
          program = pkgs.lib.getExe' librepaper "librepaper";
        };
      };
    };
}
