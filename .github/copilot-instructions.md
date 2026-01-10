# BCNelson's NixOS Configuration

This is a comprehensive multi-host NixOS configuration using Nix flakes and the Just task runner. The repository manages 15+ different host configurations including desktops, servers, VMs, and ISO images.

**Always reference these instructions first and fallback to search or bash commands only when you encounter unexpected information that does not match the info here.**

## Working Effectively

### Prerequisites & Setup
First ensure you have the required tools installed and properly configured:

```bash
# Install Nix (if not available)
sudo apt update && sudo apt install -y nix-bin

# Install Just task runner  
sudo apt install -y just

# Add user to nix-users group (required for daemon access)
sudo usermod -aG nix-users $USER

# Start nix daemon
sudo systemctl start nix-daemon

# Enable experimental features
mkdir -p ~/.config/nix
echo "experimental-features = nix-command flakes" > ~/.config/nix/nix.conf

# Switch to proper group context for nix commands
sudo -u $USER -H bash
```

### Core Validation Commands (NEVER CANCEL - Set 60+ minute timeouts)

#### Basic Structure Inspection (ALWAYS WORKS OFFLINE)
```bash
# Show flake structure - WORKS OFFLINE - 30 seconds
just --list

# View all configurations and outputs - WORKS OFFLINE - 1 minute  
NIX_CONFIG="experimental-features = nix-command flakes" nix flake show --offline

# List all host configurations - WORKS OFFLINE - 30 seconds
NIX_CONFIG="experimental-features = nix-command flakes" nix eval .#nixosConfigurations --apply builtins.attrNames --json
```

#### Build Validation (REQUIRES NETWORK - NEVER CANCEL)
```bash
# Check flake evaluation without building - 15-20 minutes - NEVER CANCEL
just check --no-build
# Alternative direct command:
NIX_CONFIG="experimental-features = nix-command flakes" nix flake check --no-build

# Full flake check with builds - 45-90 minutes - NEVER CANCEL - Set timeout to 120+ minutes
just check
# Alternative: NIX_CONFIG="experimental-features = nix-command flakes" nix flake check

# Check specific host configuration - 3-5 minutes per host - NEVER CANCEL  
just check-host sierra-2
# Alternative: nix build .#nixosConfigurations.sierra-2.config.system.build.toplevel --dry-run
```

#### Code Formatting (REQUIRES NETWORK)
```bash
# Format all Nix files with Alejandra - 5-10 minutes - NEVER CANCEL
just format
# Alternative: NIX_CONFIG="experimental-features = nix-command flakes" nix fmt
```

### Build Operations (REQUIRES NETWORK - NEVER CANCEL)

#### VM Building and Testing
```bash
# Build VM (default: vm_test) - 20-30 minutes - NEVER CANCEL - Set timeout to 60+ minutes
just build vm_test vm

# Build and test VM with QEMU - 25-35 minutes - NEVER CANCEL - Set timeout to 60+ minutes  
just test vm_test vm

# Build different machine types
just build sierra-2 toplevel  # Build system configuration
```

#### ISO Creation and Testing
```bash
# Create ISO (default: iso_desktop) - 30-45 minutes - NEVER CANCEL - Set timeout to 90+ minutes
just isoCreate iso_desktop

# Create and test ISO in QEMU - 35-50 minutes - NEVER CANCEL - Set timeout to 90+ minutes
just isoTest iso_desktop

# Install ISO to Ventoy drive (if available) - 5-10 minutes
just isoInstall iso_desktop
```

### Update Operations (REQUIRES NETWORK)

#### System Updates (Linux only) - NEVER CANCEL
```bash
# Update flake inputs - 2-5 minutes
just update

# Update NixOS system configuration - 15-30 minutes - NEVER CANCEL - Set timeout to 60+ minutes
just update-os
# Aliases: just apply, just os, just o

# Pull changes and update OS - 20-35 minutes - NEVER CANCEL - Set timeout to 60+ minutes  
just sync
```

### Secret Management
```bash
# Unlock git-crypt encrypted files (requires GPG key)
just unlock

# Lock git-crypt encrypted files  
just lock

# Regenerate age encryption keys
just rekey
```

## Validation Scenarios

### After Making Configuration Changes
1. **ALWAYS run structure validation first (works offline)**:
   ```bash
   just --list
   NIX_CONFIG="experimental-features = nix-command flakes" nix flake show --offline
   ```

2. **Test flake evaluation (requires network - NEVER CANCEL - 20+ minutes)**:
   ```bash
   just check --no-build  # Set timeout to 30+ minutes
   ```

3. **Test specific host if modifying host configs (NEVER CANCEL - 5+ minutes each)**:
   ```bash
   just check-host [hostname]  # e.g., just check-host sierra-2
   ```

4. **Format code (NEVER CANCEL - 10+ minutes)**:
   ```bash
   just format
   ```

### Before Committing Changes
**ALWAYS run these validation steps - NEVER CANCEL ANY:**

1. Format check (10+ minutes): `just format`
2. Flake evaluation (20+ minutes): `just check --no-build` 
3. If modifying specific hosts, test them: `just check-host [hostname]`

## Common Issues & Troubleshooting

### Network Connectivity Issues
- **Symptom**: `Couldn't resolve host name` errors
- **Impact**: Prevents building, but flake evaluation still works offline
- **Workaround**: Use `--offline` flag where available: `nix flake show --offline`

### Permission Issues
- **Symptom**: `Permission denied` accessing `/nix/var/nix/daemon-socket/socket`
- **Fix**: Ensure user is in `nix-users` group and daemon is running
- **Commands**:
  ```bash
  sudo usermod -aG nix-users $USER
  sudo systemctl start nix-daemon  
  sudo -u $USER -H bash  # Switch to proper group context
  ```

### Build Failures Due to Missing Dependencies
- **Symptom**: `error: path '/nix/store/...' is not valid` or `Couldn't resolve host name`
- **Cause**: Network issues preventing dependency downloads
- **Action**: Wait for network connectivity to be restored, then retry
- **Note**: Flake evaluation can still partially work, checking structure and basic validation

### Evaluation Warnings (Normal)
- **Symptom**: `trace: evaluation warning: nixvim: flake output homeManagerModules has been renamed`
- **Status**: These are warnings, not errors - evaluation continues normally
- **Action**: No action required, these are known upstream changes

### Command Alternatives When Network Limited
- **Use `--offline` flag**: `nix flake show --offline` always works
- **Use `--no-build` flag**: `nix flake check --no-build` attempts evaluation without builds
- **Structure inspection**: All structure commands work offline (just --list, nix eval)

## Architecture Overview

### Directory Structure
- **flake.nix**: Main entry point defining inputs, hosts, and outputs
- **justfile**: Task runner with common commands and aliases
- **lib/default.nix**: Helper functions including `mkHost`, `mkHome`, secret management
- **hosts/**: Host-specific configurations and data files
  - `hosts/data/`: Host configuration data (hostKey, etc.)
- **nixos/**: NixOS system configurations and mixins
  - `nixos/_mixins/roles/`: Desktop, server, workstation roles
  - `nixos/_mixins/hardware/`: Hardware-specific configurations  
  - `nixos/_mixins/users/`: Per-user system configurations
- **home-manager/**: User environment configurations
  - `home-manager/_mixins/programs/`: Application configurations
  - `home-manager/_mixins/desktops/`: Desktop environment configs
- **secrets/**: Age-encrypted secrets with agenix-rekey
- **patches/**: Nixpkgs patches for pending upstream PRs

### Available Host Configurations
```
berg-1, bravo-1, charlie-1, golf-2, golf-3, golf-4, 
iso_console, iso_desktop, redo-2, romeo-2, ryuu-2,
sierra-2, vm_test, vor-2, whiskey-1, xray-2
```

### Key Files to Check When Making Changes
- **flake.nix**: When adding/removing inputs, hosts, or channels
- **lib/default.nix**: When modifying mkHost function or secret management
- **justfile**: When adding/modifying build commands
- **hosts/data/[hostname].nix**: When adding new hosts (must include hostKey)
- **nixos/[hostname]/**: Host-specific configurations
- **.github/workflows/**: CI/CD pipeline definitions (60-minute timeouts)

### Host Configuration Pattern
Hosts use the `libx.mkHost` helper function:
```nix
"hostname" = libx.mkHost { 
  hostname = "hostname"; 
  usernames = [ "user1" "user2" ]; 
  desktop = "kde6"; # optional
  nixosMods = extraModule; # optional
  channelName = "nixpkgs-unstable"; # default
};
```

## Timing Expectations & Critical Warnings

### Command Timing (NEVER CANCEL ANY OF THESE)
- **Structure inspection (offline)**: 0.05-2 seconds (ALWAYS WORKS)
- **Flake evaluation (--no-build)**: 5-10 minutes with network, fails without - **Set timeout to 20+ minutes**
- **Full flake check**: 45-90 minutes - **Set timeout to 120+ minutes** 
- **Host configuration check**: 3-5 minutes each - **Set timeout to 10+ minutes**
- **Code formatting**: 5-10 minutes - **Set timeout to 15+ minutes**
- **VM builds**: 20-30 minutes - **Set timeout to 60+ minutes**
- **ISO creation**: 30-45 minutes - **Set timeout to 90+ minutes**
- **System updates**: 15-30 minutes - **Set timeout to 60+ minutes**

### GitHub Actions Timing
- **CI host checks**: 60-minute timeout (parallel execution)
- **Multiple hosts checked simultaneously**: Plan for full 60+ minutes

**CRITICAL**: All build and evaluation operations can take substantial time. Set appropriate timeouts and NEVER cancel long-running operations. Builds may appear to hang but are often still progressing - wait for completion.

## Common Tasks Reference

### Repository Root Contents
```bash
ls -la
# Key files and directories:
# .envrc              - direnv configuration
# .github/            - GitHub workflows and copilot instructions  
# CLAUDE.md           - Claude AI coding instructions
# flake.nix           - Main flake configuration (6226 bytes)
# flake.lock          - Dependency lock file (24013 bytes)
# justfile            - Task runner recipes (5134 bytes)
# lib/                - Helper functions and utilities
# hosts/              - Host configuration data
# nixos/              - NixOS system configurations
# home-manager/       - User environment configurations
# secrets/            - Age-encrypted secrets
# shell.nix           - Development shell configuration
# main.tf             - Terraform infrastructure
```

### Checking Flake Structure
The following output is from `nix flake show --offline` (always works):
```
├───nixosConfigurations
│   ├───berg-1: NixOS configuration  
│   ├───bravo-1: NixOS configuration
│   ├───charlie-1: NixOS configuration
│   ├───golf-2: NixOS configuration
│   ├───golf-3: NixOS configuration
│   ├───golf-4: NixOS configuration
│   ├───iso_console: NixOS configuration
│   ├───iso_desktop: NixOS configuration
│   ├───redo-2: NixOS configuration
│   ├───romeo-2: NixOS configuration
│   ├───ryuu-2: NixOS configuration
│   ├───sierra-2: NixOS configuration
│   ├───vor-2: NixOS configuration
│   ├───whiskey-1: NixOS configuration
│   └───xray-2: NixOS configuration
├───packages.x86_64-linux
│   ├───amazing-marvin: package 'amazing-marvin-1.67.1'
│   ├───claude-code: package 'claude-code-1.0.60'  
│   ├───dolphin-shred: package 'kde-shred-menu-0.1.0'
│   ├───install-system: package 'install-system-0.1.0'
│   ├───mb4-extractor: package 'm4b-extractor'
│   └───mdns-reflector: package 'mdns-reflector-...'
└───devShells.x86_64-linux
    └───default: development environment 'nix-shell'
```