# Nix Config Assistant

## Build, Test & Lint Commands
- `just update-os` - Update NixOS system (alias: `apply`, `os`, `o`)
- `just update-home` - Update home-manager config (alias: `home`, `h`)
- `just all` - Update both OS and home-manager configs
- `just update` - Update flake inputs
- `just sync` - Pull changes and update OS
- `just format` - Format Nix files with Alejandra (alias: `fmt`)
- `just check` - Run flake checks
- `just build [machine=vm_test] [type=vm]` - Build a VM or system type
- `just test [machine=vm_test] [type=vm]` - Build and test a VM
- `just isoTest [version=iso_desktop]` - Create and test ISO in QEMU
- `nix build .#nixosConfigurations.{{hostname}}.config.system.build.toplevel --dry-run` - check one host

## Code Style Guidelines
- **Formatting**: 2-space indentation, single space after colons and commas
- **Naming**: camelCase for attributes/variables, snake_case for files/directories
- **Organization**: Use _mixins directory for shared configurations
- **Imports**: Grouped at top of files, relative paths with "../" notation
- **Module Structure**: Standard pattern `{ config, pkgs, lib, ... }: { ... }`
- **Error Handling**: Default values with getSecretWithDefault, builtins.trace for warnings
- **Components**: Organized into hosts, nixos, home-manager directories

## PR Integration Patterns
When nixpkgs PRs take too long to merge, use the patches pattern:

### Method 1: Patches (Recommended)
```nix
"host" = libx.mkHost {
  patches = [{
    url = "https://patch-diff.githubusercontent.com/raw/NixOS/nixpkgs/pull/XXXXX.patch";
    hash = "sha256-...";
  }];
};
```

### Method 2: Direct PR Branch Reference (Use when patches have conflicts)
```nix
inputs = {
  nixpkgs-with-feature.url = "github:author/nixpkgs/pr-branch-name";
};
versions = {
  unstable-with-feature = {
    nixpkgs = inputs.nixpkgs-with-feature;
    home-manager = inputs.home-manager-unstable;
  };
};
```

### Troubleshooting Patch Conflicts
- Patch conflicts often occur when nixpkgs-unstable is behind master
- Wait a few days for unstable to catch up, or use Method 2 temporarily
- Use `nix-prefetch-url` to get correct patch hashes
- Monitor PR status and remove patches once merged