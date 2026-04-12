#!/bin/sh
# Savants installer
#
# curl -fsSL savants.sh | sh
# OR from astra directly (Tailscale):
# curl -fsSL http://100.95.164.99:30900/savants-releases/install.sh | sh
#
# Detects OS/arch, downloads the right binary, installs to ~/.savants/bin/

set -e

SAVANTS_HOME="${SAVANTS_HOME:-$HOME/.savants}"
BIN_DIR="$SAVANTS_HOME/bin"

# Gitea releases on astra (Tailscale network)
GITEA_URL="${SAVANTS_DOWNLOAD_URL:-https://git.bernad.in/miguel/savants/releases/download/latest}"
# MinIO on astra (Tailscale, fast direct access)
MINIO_URL="http://100.95.164.99:30900/savants-releases/latest"
# Public fallback (when savants.sh is live)
PUBLIC_URL="https://savants.dev/releases/latest"

# Colors
if [ -t 1 ]; then
    GREEN='\033[32m'; YELLOW='\033[33m'; RED='\033[31m'; BOLD='\033[1m'; RESET='\033[0m'
else
    GREEN=''; YELLOW=''; RED=''; BOLD=''; RESET=''
fi

info()  { printf "${GREEN}>${RESET} %s\n" "$*"; }
warn()  { printf "${YELLOW}!${RESET} %s\n" "$*"; }
error() { printf "${RED}x${RESET} %s\n" "$*" >&2; exit 1; }

detect_platform() {
    OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
    ARCH="$(uname -m)"
    case "$OS" in
        linux)  OS="linux" ;;
        darwin) OS="apple-darwin" ;;
        *)      error "Unsupported OS: $OS" ;;
    esac
    case "$ARCH" in
        x86_64|amd64)  ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        *)             error "Unsupported arch: $ARCH" ;;
    esac

    if [ "$OS" = "linux" ]; then
        TARGET="${ARCH}-unknown-linux-gnu"
    else
        TARGET="${ARCH}-${OS}"
    fi
    info "Platform: ${BOLD}${TARGET}${RESET}"
}

download() {
    url="$1"; dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$dest" "$url"
    else
        error "Need curl or wget"
    fi
}

main() {
    printf "\n${BOLD}  Savants Installer${RESET}\n\n"
    detect_platform
    mkdir -p "$BIN_DIR" "$SAVANTS_HOME/data"

    FILENAME="savants-${TARGET}"
    ARCHIVE="${FILENAME}.tar.gz"

    # Try Gitea releases first, then MinIO, then public
    info "Downloading..."
    if download "${GITEA_URL}/${ARCHIVE}" "/tmp/${ARCHIVE}" 2>/dev/null; then
        info "Downloaded from git.bernad.in (Gitea release)"
    elif download "${MINIO_URL}/${ARCHIVE}" "/tmp/${ARCHIVE}" 2>/dev/null; then
        info "Downloaded from astra MinIO (local network)"
    elif download "${PUBLIC_URL}/${ARCHIVE}" "/tmp/${ARCHIVE}" 2>/dev/null; then
        info "Downloaded from savants.dev"
    else
        # Last resort: build from source
        warn "No pre-built binary available. Building from source..."
        if ! command -v cargo >/dev/null 2>&1; then
            info "Installing Rust..."
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
            . "$HOME/.cargo/env"
        fi
        TMPDIR=$(mktemp -d)
        git clone https://git.bernad.in/miguel/savants.git "$TMPDIR/savants"
        cd "$TMPDIR/savants/savants-cli"
        cargo build --release
        cp target/release/savants "$BIN_DIR/savants"
        rm -rf "$TMPDIR"
        chmod +x "$BIN_DIR/savants"
        ensure_path
        print_success
        return
    fi

    # Extract
    tar xzf "/tmp/${ARCHIVE}" -C "$BIN_DIR"
    mv "$BIN_DIR/${FILENAME}" "$BIN_DIR/savants" 2>/dev/null || true
    chmod +x "$BIN_DIR/savants"
    rm -f "/tmp/${ARCHIVE}"

    ensure_path
    print_success
}

ensure_path() {
    case ":$PATH:" in
        *":$BIN_DIR:"*) return ;;
    esac
    SHELL_NAME="$(basename "$SHELL" 2>/dev/null || echo "bash")"
    case "$SHELL_NAME" in
        zsh)  RC="$HOME/.zshrc" ;;
        fish) RC="$HOME/.config/fish/config.fish" ;;
        *)    RC="$HOME/.bashrc" ;;
    esac
    if [ -f "$RC" ] && ! grep -q "savants/bin" "$RC" 2>/dev/null; then
        printf '\n# Savants\nexport PATH="%s:$PATH"\n' "$BIN_DIR" >> "$RC"
        info "Added $BIN_DIR to PATH in $RC"
    fi
    export PATH="$BIN_DIR:$PATH"
}

print_success() {
    printf "\n${GREEN}${BOLD}  Savants installed!${RESET}\n\n"
    printf "  Get started:\n"
    printf "    ${BOLD}savants up${RESET}              auto-detect + diagnose\n"
    printf "    ${BOLD}savants mcp install${RESET}     set up AI integration\n"
    printf "    ${BOLD}savants daemon start${RESET}    continuous monitoring\n"
    printf "\n"
}

main "$@"
