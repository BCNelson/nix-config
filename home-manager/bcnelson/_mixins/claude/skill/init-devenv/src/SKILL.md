---
name: init-devenv
description: Initialize a repository with direnv and devenv (flake.nix, devenv.nix, .envrc). Supports Go, Node.js, Rust, and Python.
allowed-tools:
  - Bash
  - Read
  - Write
  - Grep
  - Glob
  - AskUserQuestion
---

# Initialize Development Environment

Set up a repository with direnv and devenv configuration files for reproducible development environments.

## Workflow

1. **Detect project type** - Run `./detect-project.sh` or check for indicators manually
2. **Check existing files** - Look for flake.nix, devenv.nix, .envrc
3. **Handle conflicts** - If files exist, ask whether to overwrite
4. **Generate files** - Create all three files using appropriate template
5. **Customize** - Ask about optional additions (services, scripts, env vars)
6. **Post-generation** - Remind user about `direnv allow` and `.gitignore`

## Project Detection Indicators

| Project Type | Indicators |
|-------------|------------|
| Go | `go.mod`, `go.sum` |
| Node.js (pnpm) | `package.json` + `pnpm-lock.yaml` |
| Node.js (yarn) | `package.json` + `yarn.lock` |
| Node.js (npm) | `package.json` + `package-lock.json` |
| Rust | `Cargo.toml`, `Cargo.lock` |
| Python | `pyproject.toml`, `setup.py`, `requirements.txt`, `uv.lock` |

## Templates

### Standard flake.nix (for devenv projects)

```nix
{
  description = "PROJECT_NAME - Development Environment";

  inputs = {
    nixpkgs.url = "github:cachix/devenv-nixpkgs/rolling";
    devenv.url = "github:cachix/devenv";
  };

  nixConfig = {
    extra-trusted-public-keys = "devenv.cachix.org-1:w1cLUi8dv3hnoSPGAuibQv+f9TZLr6cv/Hm9XgU50cw=";
    extra-substituters = "https://devenv.cachix.org";
  };

  outputs = { self, nixpkgs, devenv, ... } @ inputs:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forEachSystem = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = forEachSystem (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = devenv.lib.mkShell {
            inherit inputs pkgs;
            modules = [ ./devenv.nix ];
          };
        }
      );
    };
}
```

### Standard .envrc

```bash
if ! has nix_direnv_version || ! nix_direnv_version 3.0.6; then
  source_url "https://raw.githubusercontent.com/nix-community/nix-direnv/3.0.6/direnvrc" "sha256-RYcUJaRMf8oF5LznDrlCXbkOQrywm0HDv1VjYGaJGdM="
fi

use flake
```

### Go devenv.nix

```nix
{ pkgs, lib, config, ... }:

{
  languages.go.enable = true;

  packages = with pkgs; [
    gotools
    golangci-lint
    delve
    git
  ];

  env = {
    GOPATH = "${config.env.DEVENV_STATE}/go";
    GOCACHE = "${config.env.DEVENV_STATE}/go-cache";
    GOMODCACHE = "${config.env.DEVENV_STATE}/go-mod-cache";
  };

  enterShell = ''
    echo "Go $(go version | cut -d' ' -f3)"
  '';

  git-hooks.hooks = {
    gofmt.enable = true;
    govet.enable = true;
    golangci-lint.enable = true;
  };
}
```

### Node.js (pnpm) devenv.nix

```nix
{ pkgs, ... }:
{
  languages.javascript = {
    enable = true;
    package = pkgs.nodejs_22;
    pnpm = {
      enable = true;
      install.enable = true;
    };
  };

  env.NODE_ENV = "development";

  dotenv.enable = true;

  git-hooks.hooks = {
    prettier.enable = true;
    eslint.enable = true;
  };

  enterShell = ''
    echo "Node.js $(node --version) with pnpm $(pnpm --version)"
  '';
}
```

### Node.js (yarn) devenv.nix

```nix
{ pkgs, ... }:
{
  languages.javascript = {
    enable = true;
    package = pkgs.nodejs_22;
    corepack.enable = true;
  };

  env.NODE_ENV = "development";

  dotenv.enable = true;

  # For native modules
  packages = with pkgs; [
    python3
    gnumake
    gcc
  ];

  git-hooks.hooks = {
    prettier.enable = true;
    eslint.enable = true;
  };

  enterShell = ''
    echo "Node.js $(node --version) with yarn $(yarn --version)"
  '';
}
```

### Node.js (npm) devenv.nix

```nix
{ pkgs, ... }:
{
  languages.javascript = {
    enable = true;
    package = pkgs.nodejs_22;
    npm.enable = true;
  };

  env.NODE_ENV = "development";

  dotenv.enable = true;

  git-hooks.hooks = {
    prettier.enable = true;
    eslint.enable = true;
  };

  enterShell = ''
    echo "Node.js $(node --version) with npm $(npm --version)"
  '';
}
```

### Python devenv.nix

```nix
{ pkgs, ... }:
{
  languages.python = {
    enable = true;
    uv = {
      enable = true;
      sync.enable = true;
    };
  };

  dotenv.enable = true;

  git-hooks.hooks = {
    ruff.enable = true;
    ruff-format.enable = true;
  };

  enterShell = ''
    echo "Python $(python --version)"
  '';
}
```

### Rust flake.nix (uses rust-overlay, NOT devenv)

```nix
{
  description = "PROJECT_NAME - Development Environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        rust = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rust
            pkg-config
            openssl
          ];

          shellHook = ''
            echo "Rust $(rustc --version)"
          '';
        };
      }
    );
}
```

## Optional Customizations

After generating base files, ask the user about:

1. **Services** - PostgreSQL, Redis, MySQL, etc.
   ```nix
   services.postgres = {
     enable = true;
     initialDatabases = [{ name = "mydb"; }];
   };
   ```

2. **Custom scripts** - Development shortcuts
   ```nix
   scripts.dev.exec = "npm run dev";
   scripts.test.exec = "npm test";
   ```

3. **Additional packages** - Project-specific tools
   ```nix
   packages = with pkgs; [ jq curl httpie ];
   ```

4. **Environment variables** - Project configuration
   ```nix
   env.DATABASE_URL = "postgres://localhost/mydb";
   ```

## Post-Generation Steps

After generating files, remind the user:

1. **Allow direnv**: Run `direnv allow` to activate the environment
2. **Update .gitignore**: Add these entries if not present:
   ```
   .devenv/
   .direnv/
   ```
3. **First build**: The first `direnv allow` will download dependencies (may take a few minutes)

## Guidelines

- Always check for existing files before writing
- Use the project name from `package.json`, `Cargo.toml`, `go.mod`, or `pyproject.toml` when available
- For Rust projects, use rust-overlay instead of devenv (better Rust toolchain support)
- Keep generated configs minimal - users can add complexity as needed
- Prefer latest LTS Node.js version (currently 22)
