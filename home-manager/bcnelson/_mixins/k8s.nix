{ config, pkgs, ... }:
{
  imports = [
    ./claude/mcp/kubernetes.nix
  ];

  home.sessionVariables = {
    KUBECONFIG = "${config.xdg.configHome}/kube/config";
  };

  home.packages = [
    pkgs.k9s
    pkgs.wl-clipboard
    pkgs.kubectl
    pkgs.kubernetes-helm
  ];

  programs.granted = {
    enable = true;
    enableFishIntegration = true;
  };
}
