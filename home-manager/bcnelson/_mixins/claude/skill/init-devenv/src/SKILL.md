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
   - Use only the templates in this file — do not browse other repos or fetch external examples.
5. **Customize** - Ask about optional additions (services, scripts, env vars)
6. **Post-generation** - Append `.gitignore` entries and remind user about `direnv allow`

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

### Standard flake.nix

```nix
{
  description = "PROJECT_NAME development environment";

  inputs = {
    nixpkgs.url = "github:cachix/devenv-nixpkgs/rolling";
    devenv.url = "github:cachix/devenv";
    devenv.inputs.nixpkgs.follows = "nixpkgs";
  };

  nixConfig = {
    extra-trusted-public-keys = [
      "devenv.cachix.org-1:w1cLUi8dv3hnoSPGAuibQv+f9TZLr6cv/Hm9XgU50cw="
    ];
    extra-substituters = [
      "https://devenv.cachix.org"
    ];
  };

  outputs =
    {
      self,
      nixpkgs,
      devenv,
      ...
    }@inputs:
    let
      forEachSystem = nixpkgs.lib.genAttrs [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
    in
    {
      packages = forEachSystem (system: {
        devenv-up = self.devShells.${system}.default.config.procfileScript;
        devenv-test = self.devShells.${system}.default.config.test;
      });

      devShells = forEachSystem (
        system:
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

export DEVENV_ROOT="$PWD"

use flake "path:$PWD" --no-pure-eval --impure
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

### Rust devenv.nix

```nix
{ pkgs, ... }:
{
  languages.rust = {
    enable = true;
    components = [ "rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" "rust-src" ];
  };

  packages = with pkgs; [
    pkg-config
    openssl
    git
  ];

  git-hooks.hooks = {
    rustfmt.enable = true;
    clippy.enable = true;
  };

  enterShell = ''
    echo "Rust $(rustc --version)"
  '';
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

After generating files, the skill itself writes the `.gitignore`
entries — do not just tell the user to do it. Steps:

1. **Update .gitignore**: If `.gitignore` does not exist, create it.
   If it exists and already contains `.devenv/`, skip this step to
   avoid duplicates. Otherwise append:
   ```
   # devenv / direnv
   .devenv/
   .direnv/
   .devenv-state/
   .env
   ```
2. **Tell the user to allow direnv**: Run `direnv allow` to activate
   the environment. The first activation downloads dependencies and
   may take a few minutes.

## Guidelines

- Always check for existing files before writing
- Use the project name from `package.json`, `Cargo.toml`, `go.mod`, or `pyproject.toml` when available
- Keep generated configs minimal - users can add complexity as needed
- Prefer latest LTS Node.js version (currently 22)
