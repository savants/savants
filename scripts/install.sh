#!/bin/sh
# Savants installer — https://savants.sh
# curl -fsSL savants.sh | sh
#
# Downloads the Savants CLI and its bundled FalkorDB graph engine,
# installs to ~/.savants/bin/, and adds to PATH.
#
# Supports: Linux x86_64, Linux aarch64, macOS arm64, macOS x86_64
# Requires: curl or wget, tar

set -e

SAVANTS_HOME="${SAVANTS_HOME:-$HOME/.savants}"
BIN_DIR="$SAVANTS_HOME/bin"
BASE_URL="https://savants.dev/releases/latest"

# Colors (if terminal supports it)
if [ -t 1 ]; then
    BOLD='\033[1m'
    GREEN='\033[32m'
    YELLOW='\033[33m'
    RED='\033[31m'
    RESET='\033[0m'
else
    BOLD='' GREEN='' YELLOW='' RED='' RESET=''
fi

info()  { printf "${GREEN}>${RESET} %s\n" "$*"; }
warn()  { printf "${YELLOW}!${RESET} %s\n" "$*"; }
error() { printf "${RED}x${RESET} %s\n" "$*" >&2; exit 1; }

# Detect platform
detect_platform() {
    OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
    ARCH="$(uname -m)"

    case "$OS" in
        linux)  OS="linux" ;;
        darwin) OS="macos" ;;
        *)      error "Unsupported OS: $OS" ;;
    esac

    case "$ARCH" in
        x86_64|amd64)  ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        *)             error "Unsupported architecture: $ARCH" ;;
    esac

    PLATFORM="${OS}-${ARCH}"
    info "Detected platform: ${BOLD}${PLATFORM}${RESET}"
}

# Download file (curl or wget)
download() {
    url="$1"
    dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$dest" "$url"
    else
        error "Neither curl nor wget found. Install one and retry."
    fi
}

# Main install flow
main() {
    printf "\n${BOLD}  Savants Installer${RESET}\n"
    printf "  Your infrastructure savant.\n\n"

    detect_platform

    # Create directories
    mkdir -p "$BIN_DIR"
    mkdir -p "$SAVANTS_HOME/data"

    # Download
    ARCHIVE="savants-${PLATFORM}.tar.gz"
    DOWNLOAD_URL="${BASE_URL}/savants-${PLATFORM}.tar.gz"

    info "Downloading from ${DOWNLOAD_URL}..."
    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT

    download "$DOWNLOAD_URL" "$TMPDIR/$ARCHIVE" || {
        warn "Pre-built binary not available yet for ${PLATFORM}."
        warn "Falling back to pip install..."
        pip_install
        return
    }

    # Extract
    info "Installing to ${BIN_DIR}..."
    tar -xzf "$TMPDIR/$ARCHIVE" -C "$BIN_DIR"
    chmod +x "$BIN_DIR/savants" 2>/dev/null || true

    # Verify
    if "$BIN_DIR/savants" --version >/dev/null 2>&1; then
        VERSION="$("$BIN_DIR/savants" --version 2>&1)"
        info "Installed: ${BOLD}${VERSION}${RESET}"
    else
        warn "Binary installed but may need dependencies. Trying pip fallback..."
        pip_install
        return
    fi

    ensure_path
    print_success
}

# Fallback: pip install
pip_install() {
    info "Installing via pip..."
    if command -v pip3 >/dev/null 2>&1; then
        pip3 install --user savants 2>&1 || pip3 install --user git+https://git.bernad.in/miguel/savants.git 2>&1
    elif command -v pip >/dev/null 2>&1; then
        pip install --user savants 2>&1 || pip install --user git+https://git.bernad.in/miguel/savants.git 2>&1
    else
        error "No pip found. Install Python 3.10+ and retry."
    fi

    ensure_path
    print_success
}

# Add to PATH if needed
ensure_path() {
    case ":$PATH:" in
        *":$BIN_DIR:"*) return ;;
    esac

    # Detect shell config
    SHELL_NAME="$(basename "$SHELL" 2>/dev/null || echo "bash")"
    case "$SHELL_NAME" in
        zsh)  RC="$HOME/.zshrc" ;;
        fish) RC="$HOME/.config/fish/config.fish" ;;
        *)    RC="$HOME/.bashrc" ;;
    esac

    if [ -f "$RC" ]; then
        if ! grep -q "savants/bin" "$RC" 2>/dev/null; then
            printf '\n# Savants\nexport PATH="%s:$PATH"\n' "$BIN_DIR" >> "$RC"
            info "Added ${BIN_DIR} to PATH in ${RC}"
        fi
    fi

    export PATH="$BIN_DIR:$PATH"
}

print_success() {
    printf "\n${GREEN}${BOLD}  Savants installed successfully!${RESET}\n\n"
    printf "  Get started:\n"
    printf "    ${BOLD}savants up${RESET}           # auto-detect & diagnose everything\n"
    printf "    ${BOLD}savants story${RESET}         # full diagnosis narrative\n"
    printf "    ${BOLD}savants k8s watch${RESET}     # live cluster monitoring\n"
    printf "\n  Docs: https://savants.dev\n\n"
}

main "$@"
