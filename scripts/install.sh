#!/bin/sh
# Savants installer / updater
#
# curl -fsSL savants.sh | sh
#
# Detects OS/arch, downloads the right binary, installs to ~/.savants/bin/
# Re-run the same command to update to the latest version.

set -e

SAVANTS_HOME="${SAVANTS_HOME:-$HOME/.savants}"
BIN_DIR="$SAVANTS_HOME/bin"

# R2 CDN (primary - global edge, free egress)
R2_URL="https://releases.savants.dev"
# Fallback: GitHub releases
GH_URL="https://github.com/savants-dev/savants/releases/download"

# Colors - use actual escape bytes, not printf-interpreted sequences
if [ -t 1 ] || [ -t 2 ]; then
    CYAN=$(printf '\033[36m'); GREEN=$(printf '\033[32m'); YELLOW=$(printf '\033[33m'); RED=$(printf '\033[31m')
    BOLD=$(printf '\033[1m'); DIM=$(printf '\033[2m'); RESET=$(printf '\033[0m')
else
    CYAN=''; GREEN=''; YELLOW=''; RED=''; BOLD=''; DIM=''; RESET=''
fi

info()  { printf "%s>%s %s\n" "$CYAN" "$RESET" "$*"; }
ok()    { printf "%s>%s %s\n" "$GREEN" "$RESET" "$*"; }
warn()  { printf "%s!%s %s\n" "$YELLOW" "$RESET" "$*"; }
error() { printf "%sx%s %s\n" "$RED" "$RESET" "$*" >&2; exit 1; }

detect_platform() {
    OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
    ARCH="$(uname -m)"
    case "$OS" in
        linux)  OS_TAG="unknown-linux-gnu" ;;
        darwin) OS_TAG="apple-darwin" ;;
        *)      error "Unsupported OS: $OS" ;;
    esac
    case "$ARCH" in
        x86_64|amd64)  ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        *)             error "Unsupported arch: $ARCH" ;;
    esac
    TARGET="${ARCH}-${OS_TAG}"
}

fetch() {
    url="$1"; dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --max-time 30 -o "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$dest" "$url"
    else
        error "Need curl or wget"
    fi
}

fetch_quiet() {
    url="$1"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --max-time 5 "$url" 2>/dev/null
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- "$url" 2>/dev/null
    fi
}

main() {
    printf "\n%s  savants%s %sinstaller%s\n\n" "$BOLD" "$RESET" "$DIM" "$RESET"
    detect_platform
    info "Platform: ${BOLD}${TARGET}${RESET}"

    # Check current version (if already installed)
    CURRENT_VERSION=""
    if [ -x "$BIN_DIR/savants" ]; then
        CURRENT_VERSION="$("$BIN_DIR/savants" --version 2>/dev/null | awk '{print $2}')" || true
    fi

    # Get latest version from R2
    LATEST_VERSION="$(fetch_quiet "${R2_URL}/latest/version.txt")" || true
    LATEST_VERSION="$(echo "$LATEST_VERSION" | tr -d '[:space:]')"

    if [ -n "$CURRENT_VERSION" ] && [ -n "$LATEST_VERSION" ]; then
        if [ "$CURRENT_VERSION" = "$LATEST_VERSION" ]; then
            ok "Already on latest: ${BOLD}v${CURRENT_VERSION}${RESET}"
            printf "\n"
            exit 0
        fi
        info "Updating: ${BOLD}v${CURRENT_VERSION}${RESET} -> ${BOLD}v${LATEST_VERSION}${RESET}"
    elif [ -n "$LATEST_VERSION" ]; then
        info "Installing: ${BOLD}v${LATEST_VERSION}${RESET}"
    fi

    mkdir -p "$BIN_DIR" "$SAVANTS_HOME/data"

    ARCHIVE="savants-${TARGET}.tar.gz"
    TMP_FILE="/tmp/${ARCHIVE}"

    # Try R2 first, then GitHub releases
    info "Downloading..."
    if fetch "${R2_URL}/latest/${ARCHIVE}" "$TMP_FILE" 2>/dev/null; then
        ok "Downloaded from CDN"
    elif [ -n "$LATEST_VERSION" ] && fetch "${GH_URL}/v${LATEST_VERSION}/${ARCHIVE}" "$TMP_FILE" 2>/dev/null; then
        ok "Downloaded from GitHub"
    elif fetch "${GH_URL}/latest/${ARCHIVE}" "$TMP_FILE" 2>/dev/null; then
        ok "Downloaded from GitHub (latest)"
    else
        error "Download failed. Check https://github.com/savants-dev/savants/releases"
    fi

    # Extract and install
    tar xzf "$TMP_FILE" -C "$BIN_DIR"
    # Handle both tarball layouts (flat binary or named binary)
    [ -f "$BIN_DIR/savants-${TARGET}" ] && mv "$BIN_DIR/savants-${TARGET}" "$BIN_DIR/savants"
    chmod +x "$BIN_DIR/savants"
    rm -f "$TMP_FILE"

    ensure_path

    # Verify
    INSTALLED_VERSION="$("$BIN_DIR/savants" --version 2>/dev/null | awk '{print $2}')" || true

    printf "\n%s%s  savants v%s installed%s\n\n" "$GREEN" "$BOLD" "${INSTALLED_VERSION:-?}" "$RESET"
    if [ -n "$CURRENT_VERSION" ]; then
        printf "  Updated from v%s\n\n" "$CURRENT_VERSION"
    fi
    printf "  %ssavants up%s            auto-detect + index your repo\n" "$BOLD" "$RESET"
    printf "  %ssavants serve%s         start MCP server for your AI editor\n" "$BOLD" "$RESET"
    printf "  %ssavants reindex%s       re-index after code changes\n" "$BOLD" "$RESET"
    printf "\n  %sTo update later: curl -fsSL savants.sh | sh%s\n\n" "$DIM" "$RESET"
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
        info "Added to PATH in $RC"
    fi
    export PATH="$BIN_DIR:$PATH"
}

main "$@"
