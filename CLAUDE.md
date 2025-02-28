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

## Code Style Guidelines
- **Formatting**: 2-space indentation, single space after colons and commas
- **Naming**: camelCase for attributes/variables, snake_case for files/directories
- **Organization**: Use _mixins directory for shared configurations
- **Imports**: Grouped at top of files, relative paths with "../" notation
- **Module Structure**: Standard pattern `{ config, pkgs, lib, ... }: { ... }`
- **Error Handling**: Default values with getSecretWithDefault, builtins.trace for warnings
- **Components**: Organized into hosts, nixos, home-manager directories