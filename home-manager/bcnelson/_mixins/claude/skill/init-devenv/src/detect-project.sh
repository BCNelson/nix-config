#!/usr/bin/env bash
# Detect project type and package manager
# Outputs: PROJECT_TYPE, PACKAGE_MANAGER, HAS_LOCKFILE

set -euo pipefail

PROJECT_TYPE="unknown"
PACKAGE_MANAGER="unknown"
HAS_LOCKFILE="false"

# Check for Go
if [[ -f "go.mod" ]]; then
  PROJECT_TYPE="go"
  PACKAGE_MANAGER="go"
  if [[ -f "go.sum" ]]; then
    HAS_LOCKFILE="true"
  fi
fi

# Check for Rust
if [[ -f "Cargo.toml" ]]; then
  PROJECT_TYPE="rust"
  PACKAGE_MANAGER="cargo"
  if [[ -f "Cargo.lock" ]]; then
    HAS_LOCKFILE="true"
  fi
fi

# Check for Python
if [[ -f "pyproject.toml" ]] || [[ -f "setup.py" ]] || [[ -f "requirements.txt" ]]; then
  PROJECT_TYPE="python"
  if [[ -f "uv.lock" ]]; then
    PACKAGE_MANAGER="uv"
    HAS_LOCKFILE="true"
  elif [[ -f "poetry.lock" ]]; then
    PACKAGE_MANAGER="poetry"
    HAS_LOCKFILE="true"
  elif [[ -f "Pipfile.lock" ]]; then
    PACKAGE_MANAGER="pipenv"
    HAS_LOCKFILE="true"
  elif [[ -f "requirements.txt" ]]; then
    PACKAGE_MANAGER="pip"
    HAS_LOCKFILE="true"
  else
    PACKAGE_MANAGER="pip"
  fi
fi

# Check for Node.js (check last to allow override detection)
if [[ -f "package.json" ]]; then
  # Determine package manager
  if [[ -f "pnpm-lock.yaml" ]]; then
    PROJECT_TYPE="node-pnpm"
    PACKAGE_MANAGER="pnpm"
    HAS_LOCKFILE="true"
  elif [[ -f "yarn.lock" ]]; then
    PROJECT_TYPE="node-yarn"
    PACKAGE_MANAGER="yarn"
    HAS_LOCKFILE="true"
  elif [[ -f "package-lock.json" ]]; then
    PROJECT_TYPE="node-npm"
    PACKAGE_MANAGER="npm"
    HAS_LOCKFILE="true"
  else
    # No lockfile, check packageManager field in package.json
    if command -v jq &>/dev/null && [[ -f "package.json" ]]; then
      PM_FIELD=$(jq -r '.packageManager // empty' package.json 2>/dev/null || true)
      if [[ "$PM_FIELD" == pnpm* ]]; then
        PROJECT_TYPE="node-pnpm"
        PACKAGE_MANAGER="pnpm"
      elif [[ "$PM_FIELD" == yarn* ]]; then
        PROJECT_TYPE="node-yarn"
        PACKAGE_MANAGER="yarn"
      else
        PROJECT_TYPE="node-npm"
        PACKAGE_MANAGER="npm"
      fi
    else
      PROJECT_TYPE="node-npm"
      PACKAGE_MANAGER="npm"
    fi
  fi
fi

echo "PROJECT_TYPE=$PROJECT_TYPE"
echo "PACKAGE_MANAGER=$PACKAGE_MANAGER"
echo "HAS_LOCKFILE=$HAS_LOCKFILE"
