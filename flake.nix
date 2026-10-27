{
  description = "forest — worktree & forest manager";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: rec {
        forest = pkgs.callPackage ./nix/package.nix { };
        default = forest;
      });

      overlays.default = final: _prev: { forest = final.callPackage ./nix/package.nix { }; };

      homeManagerModules.forest = import ./nix/home-manager.nix self;
    };
}
