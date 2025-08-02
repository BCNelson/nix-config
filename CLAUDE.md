# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build, Test & Lint Commands

- `just update-os` - Update NixOS system (alias: `apply`, `os`, `o`)
- `just update-home` - Update home-manager config (alias: `home`, `h`)
- `just all` - Update both OS and home-manager configs
- `just update` - Update flake inputs
- `just sync` - Pull changes and update OS
- `just format` - Format Nix files with Alejandra (alias: `fmt`)
- `just check` - Run flake checks
- `just check-host [hostname]` - Check specific host configuration
- `just build [machine=vm_test] [type=vm]` - Build a VM or system type
- `just test [machine=vm_test] [type=vm]` - Build and test a VM
- `just isoTest [version=iso_desktop]` - Create and test ISO in QEMU
- `just unlock` - Unlock git-crypt encrypted files
- `just rekey` - Regenerate age encryption keys
- `nix build .#nixosConfigurations.{{hostname}}.config.system.build.toplevel --dry-run` - Check one host

## Architecture Overview

This is a multi-host NixOS configuration using flake-utils-plus with the following structure:

### Core Components
- **flake.nix**: Main entry point defining inputs, hosts, and outputs
- **lib/default.nix**: Helper functions including `mkHost`, `mkHome`, secret management
- **hosts/**: Host-specific configurations and data files
- **nixos/**: NixOS system configurations and mixins
- **home-manager/**: User environment configurations
- **secrets/**: Age-encrypted secrets with agenix-rekey

### Host Configuration Pattern
Hosts are defined using the `libx.mkHost` helper function:
```nix
"hostname" = libx.mkHost { 
  hostname = "hostname"; 
  usernames = [ "user1" "user2" ]; 
  desktop = "kde6"; # optional
  nixosMods = extraModule; # optional
  channelName = "nixpkgs-unstable"; # default
};
```

### Mixin System
- `nixos/_mixins/`: Shared NixOS configurations organized by category
  - `roles/`: Desktop, server, workstation roles
  - `hardware/`: Hardware-specific configurations
  - `users/`: Per-user system configurations
- `home-manager/_mixins/`: Shared home-manager configurations
  - `programs/`: Application configurations
  - `desktops/`: Desktop environment configs
  - User-specific mixins under `bcnelson/_mixins/`

### Channel Management
- Multiple nixpkgs channels available: `nixpkgs`, `nixpkgs-unstable`, `nixpkgs-unstable-small`
- Patches applied via `channels.nixpkgs-unstable-small-patched`
- Home-manager always uses unstable channel regardless of host channel

### Secret Management
- Uses agenix with FIDO2 hardware keys and age-plugin-fido2-hmac
- Secrets stored in `secrets/` with per-host directories
- Helper functions: `getSecret`, `getSecretWithDefault` for fallback values

## Code Style Guidelines

- **Formatting**: 2-space indentation, single space after colons and commas
- **Naming**: camelCase for attributes/variables, snake_case for files/directories
- **Organization**: Use _mixins directory for shared configurations
- **Imports**: Grouped at top of files, relative paths with "../" notation
- **Module Structure**: Standard pattern `{ config, pkgs, lib, ... }: { ... }`
- **Error Handling**: Default values with getSecretWithDefault, builtins.trace for warnings
- **Host Structure**: hostname-based file naming (e.g., `sierra-2.nix` loads `sierra.nix` via prefix matching)

## PR Integration Patterns

When nixpkgs PRs take too long to merge, use the patches pattern:

### Method 1: Patches (Recommended)

Apply patches at the channel level in flake.nix:
```nix
channels.nixpkgs-unstable-small-patched = {
  input = inputs.nixpkgs-unstable-small;
  patches = [ ./patches/405787.patch ];
};
```

### Method 2: Direct PR Branch Reference (Use when patches have conflicts)

```nix
inputs = {
  nixpkgs-with-feature.url = "github:author/nixpkgs/pr-branch-name";
};
# Then use as channelName in mkHost
```

### Troubleshooting Patch Conflicts

- Patch conflicts often occur when nixpkgs-unstable is behind master
- Wait a few days for unstable to catch up, or use Method 2 temporarily
- Use `nix-prefetch-url` to get correct patch hashes
- Monitor PR status and remove patches once merged

## Development Workflow

1. **Adding New Hosts**: 
   - Create `hosts/data/hostname.nix` with hostKey
   - Add entry to `flake.nix` hosts section using `libx.mkHost`
   - Create NixOS config in `nixos/hostname/` directory

2. **Adding New Users**:
   - Create user directory in `home-manager/username/`
   - Add NixOS user config in `nixos/_mixins/users/username/`

3. **Testing Changes**:
   - Use `just check` for flake validation
   - Use `just check-host hostname` for specific host testing
   - Use VM testing with `just test` for safe validation
