{ config, pkgs, lib, ... }:

let
  name = "amacs";
  ports = [
    80
    443
  ];
in
{
  imports = [ ./hardware.nix ];

  services.nginx = {
    enable = true;
    virtualHosts.${name} = {
      root = "/var/www";
    };
  };

  environment.systemPackages = with pkgs; [
    git
    ripgrep
  ];

  # A comment.
  users.users.albin = lib.mkIf config.enable {
    isNormalUser = true;
  };
}
