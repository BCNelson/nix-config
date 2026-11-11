{ pkgs, inputs, ... }:

let
  zedPkg = pkgs.unstable.zed-editor;
  # Zed renders via wgpu/Vulkan. nixGL's `mesa` wrapper only sets up OpenGL
  # env vars (LIBGL_DRIVERS_PATH etc.) and does NOT set VK_ICD_FILENAMES, so
  # on non-NixOS the Vulkan loader scans /usr/share/vulkan/icd.d and tries to
  # load Fedora's ICDs against Nix's library set — which fails. nixVulkanIntel
  # supplies a Nix-built Mesa Vulkan ICD plus libdrm/wayland/etc. on
  # LD_LIBRARY_PATH, which is what wgpu actually needs.
  nixVulkan = inputs.nixgl.packages.${pkgs.stdenv.hostPlatform.system}.nixVulkanIntel;
  zedPackage = pkgs.symlinkJoin {
    name = "${zedPkg.pname}-nixvk-${zedPkg.version}";
    paths = [ zedPkg ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    postBuild = ''
      for bin in $out/bin/*; do
        target=$(readlink -f "$bin")
        rm "$bin"
        makeWrapper ${nixVulkan}/bin/nixVulkanIntel "$bin" \
          --argv0 "$(basename "$bin")" \
          --add-flags "$target"
      done
    '';
    inherit (zedPkg) meta;
  };
in
{
  programs.zed-editor = {
    enable = true;
    package = zedPackage;
    mutableUserSettings = true;
    userSettings = {
      terminal = {
        shell = {
          program = "fish";
        };
      };
      buffer_font_family = "Monaspace Neon";
      buffer_font_size = 14;
      ui_font_size = 16;
      lsp.tailwindcss-language-server.settings.files.exclude = [
        # Tailwind's defaults; setting the list replaces them.
        "**/.git/**"
        "**/node_modules/**"
        "**/.hg/**"
        "**/.svn/**"
        # Bazel's convenience symlinks (bazel-bin, bazel-out, bazel-<ws>, ...)
        # point into the output base. The server follows them while globbing
        # for Tailwind configs, and in the redo monorepo that walk ran node
        # out of heap within two minutes of opening the project.
        "**/bazel-*/**"
        "**/target/**"
      ];
      language_models = {
        ollama = {
          api_url = "http://romeo.b.nel.family:11434";
          auto_discover = true;
        };
      };
    };
  };
}
