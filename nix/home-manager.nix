# programs.forest on home-manager: the forest CLI on the user's PATH.
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.forest;
  system = pkgs.stdenv.hostPlatform.system;
in
{
  options.programs.forest = {
    enable = lib.mkEnableOption "forest";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${system}.forest;
      defaultText = lib.literalExpression "forest.packages.\${system}.forest";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
  };
}
