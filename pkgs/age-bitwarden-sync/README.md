# age-bitwarden-sync

A tool to synchronize age-encrypted secrets from a NixOS configuration to Bitwarden, enabling secure secret management across multiple hosts using hardware security keys.

## Features

- **Automatic Secret Discovery**: Scans NixOS flake configurations to find all age-encrypted secrets
- **Hardware Key Support**: Works with FIDO2 security keys (YubiKey) for secure decryption
- **Selective Sync**: Sync all secrets or filter by specific hosts
- **Bitwarden Integration**: Creates secure notes in Bitwarden with decrypted secret content
- **Duplicate Prevention**: Checks for existing items before creating new ones

## Installation

This package is included in the NixOS configuration. It can be built using:

```bash
nix build .#age-bitwarden-sync
```

## Usage

### Basic Usage

Sync all secrets using a FIDO key:
```bash
age-bitwarden-sync --fido --password <bitwarden-password>
```

### Options

- `-f, --fido` - Use FIDO key for decryption (no identity file needed)
- `-i, --identity <path>` - Path to age identity file (optional when using FIDO keys)
- `-p, --password <password>` - Bitwarden password (or use BW_PASSWORD env var)
- `-H, --host <hostname>` - Only sync secrets from specific host(s), can be specified multiple times
- `--flake <path>` - Path to the flake (defaults to current directory)
- `-v, --verbose` - Enable verbose output

### Examples

Sync secrets for specific hosts:
```bash
age-bitwarden-sync --fido --host sierra --host romeo
```

Use environment variable for password:
```bash
export BW_PASSWORD="your-password"
age-bitwarden-sync --fido
```

Use a specific age identity file:
```bash
age-bitwarden-sync --identity ~/.age/key.txt --password <password>
```

## How It Works

1. **Secret Discovery**: The tool evaluates your NixOS flake configuration to find all age-encrypted secret files
2. **Decryption**: Uses age with either FIDO keys or identity files to decrypt the secrets
3. **Bitwarden Sync**: Logs into Bitwarden CLI and creates secure notes for each secret
4. **Organization**: Secrets are named using the pattern `<hostname>/<secret-name>` for easy identification

## Requirements

- `bitwarden-cli` - For Bitwarden operations
- `age` - For secret decryption
- `age-plugin-yubikey` - For YubiKey support
- `age-plugin-fido2-hmac` - For FIDO2 hardware key support
- `nix` - For evaluating flake configurations
- `jq` - For JSON processing

All dependencies are automatically provided when built through Nix.

## Security Considerations

- Secrets are only decrypted temporarily in memory during sync
- Bitwarden master password is never stored on disk
- FIDO keys provide hardware-based security for decryption
- Each secret is stored as a secure note in Bitwarden with appropriate metadata

## License

MIT