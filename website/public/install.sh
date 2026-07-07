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
GH_URL="https://github.com/savants/savants/releases/download"

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

is_nixos() {
    [ -f /etc/NIXOS ] || [ -d /nix/store ]
}

install_nixos() {
    printf "\n%s  savants%s %sinstaller (NixOS)%s\n\n" "$BOLD" "$RESET" "$DIM" "$RESET"
    info "NixOS detected"

    mkdir -p "$BIN_DIR" "$SAVANTS_HOME/data"

    # Strategy: try musl static binary first (no patching), then glibc+patchelf, then source
    detect_platform

    # Try 1: musl static binary (works on NixOS without any patching)
    MUSL_TARGET="${ARCH}-unknown-linux-musl"
    MUSL_ARCHIVE="savants-${MUSL_TARGET}.tar.gz"
    MUSL_TMP="/tmp/${MUSL_ARCHIVE}"

    info "Trying static binary (musl)..."
    if fetch "${R2_URL}/latest/${MUSL_ARCHIVE}" "$MUSL_TMP" 2>/dev/null; then
        rm -f "$BIN_DIR/savants.old" 2>/dev/null
        mv "$BIN_DIR/savants" "$BIN_DIR/savants.old" 2>/dev/null || true
        tar xzf "$MUSL_TMP" -C "$BIN_DIR"
        # Rename whatever was extracted to 'savants'
        for f in "$BIN_DIR"/savants-*; do [ -f "$f" ] && mv "$f" "$BIN_DIR/savants"; done
        chmod +x "$BIN_DIR/savants" 2>/dev/null
        rm -f "$MUSL_TMP"

        if "$BIN_DIR/savants" --version >/dev/null 2>&1; then
            ok "Static binary works"
            ensure_path
            INSTALLED_VERSION="$("$BIN_DIR/savants" --version 2>/dev/null | cut -d' ' -f2)" || true
            setup_guard > /dev/null 2>&1
            info "[✓] Guard protection activated"
            printf "\n%s%s  savants v%s installed%s\n" "$GREEN" "$BOLD" "${INSTALLED_VERSION:-?}" "$RESET"
            printf "  %sInstalled to: %s%s\n\n" "$DIM" "$BIN_DIR/savants" "$RESET"
            printf "  %ssavants guard list%s     see active guard rules\n" "$BOLD" "$RESET"
            printf "  %ssavants guard stats%s    see what got blocked\n" "$BOLD" "$RESET"
            printf "  %ssavants up%s             index your repo for code intelligence\n" "$BOLD" "$RESET"
            printf "\n  %sCustomize: savants guard preset battle-tested%s\n" "$DIM" "$RESET"
            printf "  %sTo update:  curl -fsSL savants.sh | sh%s\n\n" "$DIM" "$RESET"
            print_path_notice
            return
        else
            info "Static binary not compatible, trying glibc+patchelf..."
            rm -f "$BIN_DIR/savants" 2>/dev/null
            mv "$BIN_DIR/savants.old" "$BIN_DIR/savants" 2>/dev/null || true
        fi
    fi

    # Try 2: glibc binary + patchelf
    GLIBC_TARGET="${ARCH}-unknown-linux-gnu"
    GLIBC_ARCHIVE="savants-${GLIBC_TARGET}.tar.gz"
    GLIBC_TMP="/tmp/${GLIBC_ARCHIVE}"

    info "Trying precompiled binary (glibc+patchelf)..."
    if fetch "${R2_URL}/latest/${GLIBC_ARCHIVE}" "$GLIBC_TMP" 2>/dev/null; then
        rm -f "$BIN_DIR/savants.old" 2>/dev/null
        mv "$BIN_DIR/savants" "$BIN_DIR/savants.old" 2>/dev/null || true
        tar xzf "$GLIBC_TMP" -C "$BIN_DIR"
        for f in "$BIN_DIR"/savants-*; do [ -f "$f" ] && mv "$f" "$BIN_DIR/savants"; done
        chmod +x "$BIN_DIR/savants"
        rm -f "$GLIBC_TMP"

        # NixOS needs patchelf to fix the dynamic linker path
        info "Patching binary for NixOS (patchelf)..."
        INTERP=$(nix-shell -p glibc --run "cat \$(nix path-info nixpkgs#glibc)/nix-support/dynamic-linker" 2>/dev/null || \
                 find /nix/store -name "ld-linux-x86-64.so.2" -path "*/glibc-*/lib/*" 2>/dev/null | head -1)

        if [ -n "$INTERP" ] && command -v patchelf >/dev/null 2>&1; then
            RPATH=$(nix-shell -p openssl zlib stdenv.cc.cc --run 'echo $NIX_LD_LIBRARY_PATH' 2>/dev/null || \
                    echo "$(dirname "$INTERP"):$(find /nix/store -maxdepth 2 -name "libssl.so*" -printf '%h\n' 2>/dev/null | head -1):$(find /nix/store -maxdepth 2 -name "libz.so*" -printf '%h\n' 2>/dev/null | head -1)")
            patchelf --set-interpreter "$INTERP" --set-rpath "$RPATH" "$BIN_DIR/savants" 2>/dev/null
        elif command -v nix-shell >/dev/null 2>&1; then
            nix-shell -p patchelf glibc openssl zlib stdenv.cc.cc --run "
                INTERP=\$(cat \$(nix path-info nixpkgs#glibc)/nix-support/dynamic-linker 2>/dev/null || find /nix/store -name 'ld-linux-x86-64.so.2' -path '*/glibc-*/lib/*' | head -1)
                RPATH=\$NIX_LD_LIBRARY_PATH
                patchelf --set-interpreter \"\$INTERP\" --set-rpath \"\$RPATH\" '$BIN_DIR/savants'
            " 2>/dev/null
        fi

        # Verify the patched binary works
        if "$BIN_DIR/savants" --version >/dev/null 2>&1; then
            ok "Binary patched and working"
            ensure_path
            INSTALLED_VERSION="$("$BIN_DIR/savants" --version 2>/dev/null | cut -d' ' -f2)" || true

            setup_guard > /dev/null 2>&1
            info "[✓] Guard protection activated"

            printf "\n%s%s  savants v%s installed%s\n" "$GREEN" "$BOLD" "${INSTALLED_VERSION:-?}" "$RESET"
            printf "  %sInstalled to: %s%s\n\n" "$DIM" "$BIN_DIR/savants" "$RESET"

            printf "  %ssavants guard list%s     see active guard rules\n" "$BOLD" "$RESET"
            printf "  %ssavants guard stats%s    see what got blocked\n" "$BOLD" "$RESET"
            printf "  %ssavants up%s             index your repo for code intelligence\n" "$BOLD" "$RESET"
            printf "\n  %sCustomize: savants guard preset battle-tested%s\n" "$DIM" "$RESET"
            printf "  %sTo update:  curl -fsSL savants.sh | sh%s\n\n" "$DIM" "$RESET"
            print_path_notice
            return
        else
            warn "Patched binary doesn't work, building from source..."
            rm -f "$BIN_DIR/savants" 2>/dev/null
            mv "$BIN_DIR/savants.old" "$BIN_DIR/savants" 2>/dev/null || true
        fi
    else
        info "No precompiled binary, building from source..."
    fi

    # Fallback: build from source
    if ! command -v nix-shell >/dev/null 2>&1; then
        error "nix-shell not found"
    fi

    if [ -x "$BIN_DIR/savants" ]; then
        CURRENT_VERSION="$("$BIN_DIR/savants" --version 2>/dev/null | cut -d' ' -f2)" || true
        info "Current: ${BOLD}v${CURRENT_VERSION}${RESET}"
        info "Updates typically take ${BOLD}~30 seconds${RESET}"
    else
        info "First install takes ${BOLD}~10-15 minutes${RESET} (building from source)"
    fi

    SRC_DIR="/tmp/savants-src"
    if [ -d "$SRC_DIR/.git" ]; then
        # Check if layout is correct (savants-cli/ subdir exists)
        if [ -d "$SRC_DIR/savants-cli" ]; then
            info "Updating source..."
            git -C "$SRC_DIR" pull --quiet --tags 2>/dev/null || true
        else
            # Stale clone with old layout — re-clone
            info "Re-cloning (repo structure changed)..."
            rm -rf "$SRC_DIR"
            git clone --quiet --depth 1 --tags https://github.com/savants/savants.git "$SRC_DIR"
        fi
    else
        info "Cloning source..."
        rm -rf "$SRC_DIR"
        git clone --quiet --depth 1 --tags https://github.com/savants/savants.git "$SRC_DIR"
    fi

    printf "\n"

    # Count total crates for progress (from Cargo.lock)
    TOTAL_CRATES=0
    if [ -f "$SRC_DIR/savants-cli/Cargo.lock" ]; then
        TOTAL_CRATES=$(grep -c '^\[\[package\]\]' "$SRC_DIR/savants-cli/Cargo.lock" 2>/dev/null || echo 250)
    fi
    [ "$TOTAL_CRATES" -eq 0 ] && TOTAL_CRATES=250

    info "[1/4] Resolving dependencies...\n"

    nix-shell -p pkg-config openssl cmake --extra-experimental-features flakes \
        --run "cd $SRC_DIR/savants-cli && cargo build --release 2>&1" | \
    {
        COMPILED=0
        LAST_PCT=0
        START_TIME=$(date +%s)
        while IFS= read -r line; do
            case "$line" in
                *Compiling*)
                    COMPILED=$((COMPILED + 1))
                    CRATE=$(echo "$line" | sed 's/.*Compiling //' | cut -d' ' -f1)
                    PCT=$((COMPILED * 100 / TOTAL_CRATES))
                    [ "$PCT" -gt 100 ] && PCT=100

                    # Show EVERY crate in real-time — SOTA visibility
                    NOW=$(date +%s)
                    ELAPSED=$((NOW - START_TIME))
                    ETA=""
                    if [ "$COMPILED" -gt 5 ] && [ "$ELAPSED" -gt 0 ]; then
                        RATE=$((COMPILED * 100 / ELAPSED))
                        if [ "$RATE" -gt 0 ]; then
                            REMAINING=$(( (TOTAL_CRATES - COMPILED) * 100 / RATE ))
                            if [ "$REMAINING" -gt 60 ]; then
                                ETA="  ${DIM}~$(( REMAINING / 60 ))m $(( REMAINING % 60 ))s${RESET}"
                            else
                                ETA="  ${DIM}~${REMAINING}s${RESET}"
                            fi
                        fi
                    fi

                    printf "  %s>%s [2/4] %s%3d%s/%d  %s%-30s%s%s\n" \
                        "$CYAN" "$RESET" "$BOLD" "$COMPILED" "$RESET" "$TOTAL_CRATES" \
                        "$DIM" "$CRATE" "$RESET" "$ETA"
                    ;;
                *Finished*)
                    printf "  %s>%s [2/4] Compiled %d crates %s✓%s\n" \
                        "$GREEN" "$RESET" "$COMPILED" "$GREEN" "$RESET"
                    ;;
            esac
        done
    }
    info "[3/4] Installing binary..."

    if [ -f "$SRC_DIR/savants-cli/target/release/savants" ]; then
        rm -f "$BIN_DIR/savants.old" 2>/dev/null
        mv "$BIN_DIR/savants" "$BIN_DIR/savants.old" 2>/dev/null || true
        cp "$SRC_DIR/savants-cli/target/release/savants" "$BIN_DIR/savants"
        chmod +x "$BIN_DIR/savants"
        ensure_path
        INSTALLED_VERSION="$("$BIN_DIR/savants" --version 2>/dev/null | cut -d' ' -f2)" || true
        # Auto-setup guard protection (suppress output, show our own)
        setup_guard > /dev/null 2>&1
        info "[4/4] Guard protection activated"

        printf "\n%s%s  savants v%s installed%s\n" "$GREEN" "$BOLD" "${INSTALLED_VERSION:-?}" "$RESET"
        printf "  %sInstalled to: %s%s\n\n" "$DIM" "$BIN_DIR/savants" "$RESET"

        printf "  %ssavants guard list%s     see active guard rules\n" "$BOLD" "$RESET"
        printf "  %ssavants guard stats%s    see what got blocked\n" "$BOLD" "$RESET"
        printf "  %ssavants up%s             index your repo for code intelligence\n" "$BOLD" "$RESET"
        printf "\n  %sCustomize: savants guard preset battle-tested%s\n" "$DIM" "$RESET"
        printf "  %sTo update:  curl -fsSL savants.sh | sh%s\n\n" "$DIM" "$RESET"
        print_path_notice
    else
        error "Build failed. Run: cd /tmp/savants-src/savants-cli && nix-shell -p pkg-config openssl cmake --run 'cargo build --release'"
    fi
}

main() {
    # NixOS: dynamically linked binaries don't work, use nix flake instead
    if is_nixos; then
        install_nixos
        return
    fi

    printf "\n%s  savants%s %sinstaller%s\n\n" "$BOLD" "$RESET" "$DIM" "$RESET"
    detect_platform

    # Check current version (if already installed)
    CURRENT_VERSION=""
    if [ -x "$BIN_DIR/savants" ]; then
        CURRENT_VERSION="$("$BIN_DIR/savants" --version 2>/dev/null | cut -d' ' -f2)" || true
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
    fi

    mkdir -p "$BIN_DIR" "$SAVANTS_HOME/data"

    ARCHIVE="savants-${TARGET}.tar.gz"
    TMP_FILE="/tmp/${ARCHIVE}"

    VERSION_LABEL="${LATEST_VERSION:-latest}"
    printf "  ${DIM}[1/4]${RESET} Detecting platform: ${BOLD}${TARGET}${RESET}\n"

    printf "  ${DIM}[2/4]${RESET} Downloading v${VERSION_LABEL}..."
    if fetch "${R2_URL}/latest/${ARCHIVE}" "$TMP_FILE" 2>/dev/null; then
        printf " ${GREEN}done${RESET}\n"
    elif [ -n "$LATEST_VERSION" ] && fetch "${GH_URL}/v${LATEST_VERSION}/${ARCHIVE}" "$TMP_FILE" 2>/dev/null; then
        printf " ${GREEN}done${RESET}\n"
    elif fetch "${GH_URL}/latest/${ARCHIVE}" "$TMP_FILE" 2>/dev/null; then
        printf " ${GREEN}done${RESET}\n"
    else
        printf " ${RED}failed${RESET}\n"
        error "Download failed. Check https://github.com/savants/savants/releases"
    fi

    printf "  ${DIM}[3/4]${RESET} Installing binary..."
    rm -f "$BIN_DIR/savants.old" 2>/dev/null
    mv "$BIN_DIR/savants" "$BIN_DIR/savants.old" 2>/dev/null || true
    tar xzf "$TMP_FILE" -C "$BIN_DIR"
    for f in "$BIN_DIR"/savants-*; do [ -f "$f" ] && mv "$f" "$BIN_DIR/savants"; done
    chmod +x "$BIN_DIR/savants"
    rm -f "$TMP_FILE"
    printf " ${GREEN}done${RESET}\n"

    ensure_path

    printf "  ${DIM}[4/4]${RESET} Setting up guard protection..."

    # Verify installed version matches expected version
    INSTALLED_VERSION="$("$BIN_DIR/savants" --version 2>/dev/null | cut -d' ' -f2)" || true

    if [ -n "$LATEST_VERSION" ] && [ -n "$INSTALLED_VERSION" ] && [ "$INSTALLED_VERSION" != "$LATEST_VERSION" ]; then
        printf " ${RED}VERSION MISMATCH${RESET}\n"
        warn "Expected v${LATEST_VERSION} but binary reports v${INSTALLED_VERSION}"
        warn "The binary on the CDN is outdated. Restoring previous version."
        # Restore old binary
        if [ -f "$BIN_DIR/savants.old" ]; then
            mv "$BIN_DIR/savants.old" "$BIN_DIR/savants"
            warn "Restored previous binary. Please report this issue."
        fi
        exit 1
    fi

    # Auto-setup guard protection (suppress output, we show our own)
    setup_guard > /dev/null 2>&1
    printf " ${GREEN}done${RESET}\n"

    printf "\n%s%s  savants v%s installed%s\n" "$GREEN" "$BOLD" "${INSTALLED_VERSION:-?}" "$RESET"
    printf "  %sInstalled to: %s%s\n" "$DIM" "$BIN_DIR/savants" "$RESET"
    if [ -n "$CURRENT_VERSION" ]; then
        printf "  %sUpdated from v%s%s\n" "$DIM" "$CURRENT_VERSION" "$RESET"
    fi

    printf "\n"
    printf "  %ssavants guard list%s     see active guard rules\n" "$BOLD" "$RESET"
    printf "  %ssavants guard stats%s    see what got blocked\n" "$BOLD" "$RESET"
    printf "  %ssavants up%s             index your repo for code intelligence\n" "$BOLD" "$RESET"
    printf "\n  %sCustomize: savants guard preset battle-tested%s\n" "$DIM" "$RESET"
    printf "  %sTo update:  curl -fsSL savants.sh | sh%s\n\n" "$DIM" "$RESET"
    print_path_notice
}

setup_guard() {
    # Auto-detect AI editors and install guard protection
    GUARD_RULES_FILE="$SAVANTS_HOME/guard-rules.json"
    DETECTED=""

    # Standard guard preset (25 rules) — embedded so no external file needed
    # Mix of block (hard stop), suggest (LLM auto-recovers), rewrite (silent swap), ask (user approval)
    STANDARD_RULES='[
  "when tool eq '\''Bash'\'' and command contains '\''rm -rf /'\'' then block",
  "when tool eq '\''Bash'\'' and command contains '\''rm -rf ~'\'' then block",
  "when tool eq '\''Bash'\'' and command contains '\''rm -rf $HOME'\'' then block",
  "when tool eq '\''Bash'\'' and command contains '\''rm -rf .'\'' then suggest '\''Use git clean -fd for tracked repos, or remove specific files instead of rm -rf .'\''",
  "when tool eq '\''Bash'\'' and command contains '\''sudo rm'\'' then ask '\''sudo rm is destructive and irreversible'\''",
  "when tool eq '\''Bash'\'' and command contains '\''chmod 777'\'' then suggest '\''Use chmod 755 for directories or chmod 644 for files instead of 777'\''",
  "when tool eq '\''Bash'\'' and command contains '\''mkfs'\'' then block",
  "when tool eq '\''Bash'\'' and command contains '\''dd if='\'' then block",
  "when tool eq '\''Bash'\'' and command contains '\''git push --force'\'' then rewrite '\''git push --force-with-lease'\''",
  "when tool eq '\''Bash'\'' and command contains '\''git push -f '\'' then rewrite '\''git push --force-with-lease'\''",
  "when tool eq '\''Bash'\'' and command contains '\''git reset --hard'\'' then suggest '\''Use git stash to save changes before resetting, or git reset --soft to keep changes staged'\''",
  "when tool eq '\''Bash'\'' and command contains '\''DROP DATABASE'\'' then block",
  "when tool eq '\''Bash'\'' and command contains '\''DROP TABLE'\'' then ask '\''DROP TABLE is irreversible — are you sure?'\''",
  "when tool eq '\''Bash'\'' and command contains '\''TRUNCATE TABLE'\'' then ask '\''TRUNCATE TABLE deletes all rows — consider DELETE with WHERE instead'\''",
  "when tool eq '\''Bash'\'' and command contains '\''npm publish'\'' then ask '\''Publishing to npm is public and permanent'\''",
  "when tool eq '\''Bash'\'' and command contains '\''docker push'\'' then ask '\''Pushing a Docker image to a registry'\''",
  "when tool eq '\''Bash'\'' and command contains '\''terraform destroy'\'' then block",
  "when tool eq '\''Bash'\'' and command contains '\''kubectl delete namespace'\'' then block",
  "when tool eq '\''Bash'\'' and command contains '\''curl'\'' and command contains '\''| sh'\'' then block",
  "when tool eq '\''Bash'\'' and command contains '\''curl'\'' and command contains '\''| bash'\'' then block",
  "when tool eq '\''Write'\'' and file_path contains '\''.env'\'' then ask '\''Writing to .env may expose secrets if committed. Add .env to .gitignore first'\''",
  "when tool eq '\''Write'\'' and file_path contains '\''credentials'\'' then block",
  "when tool eq '\''Write'\'' and file_path contains '\''id_rsa'\'' then block",
  "when tool eq '\''Write'\'' and file_path contains '\''.ssh'\'' then block",
  "when tool eq '\''Edit'\'' and file_path contains '\''.env'\'' then ask '\''Editing .env may expose secrets. Verify .env is in .gitignore before proceeding'\''"
]'

    # Write guard rules
    echo "$STANDARD_RULES" > "$GUARD_RULES_FILE"

    # Claude Code: ~/.claude/settings.json
    CLAUDE_DIR="$HOME/.claude"
    if [ -d "$CLAUDE_DIR" ] || command -v claude >/dev/null 2>&1; then
        DETECTED="${DETECTED}Claude Code, "
        CLAUDE_SETTINGS="$CLAUDE_DIR/settings.json"
        mkdir -p "$CLAUDE_DIR"
        INTERCEPT_CMD="$BIN_DIR/savants hook intercept"

        # Create or update settings.json with PreToolUse hooks
        if [ -f "$CLAUDE_SETTINGS" ]; then
            # Add hooks if not already present
            if ! grep -q "savants hook intercept" "$CLAUDE_SETTINGS" 2>/dev/null; then
                python3 -c "
import json, sys
settings = json.load(open('$CLAUDE_SETTINGS'))
hooks = settings.setdefault('hooks', {})
pre = hooks.setdefault('PreToolUse', [])
# Remove old savants hooks
pre = [h for h in pre if 'savants' not in json.dumps(h)]
for tool in ['Grep', 'Bash', 'Read']:
    pre.append({'matcher': tool, 'hooks': [{'type': 'command', 'command': '$INTERCEPT_CMD'}]})
hooks['PreToolUse'] = pre
json.dump(settings, open('$CLAUDE_SETTINGS', 'w'), indent=2)
" 2>/dev/null || true
            fi
        else
            cat > "$CLAUDE_SETTINGS" << SETTINGS
{
  "hooks": {
    "PreToolUse": [
      {"matcher": "Grep", "hooks": [{"type": "command", "command": "$INTERCEPT_CMD"}]},
      {"matcher": "Bash", "hooks": [{"type": "command", "command": "$INTERCEPT_CMD"}]},
      {"matcher": "Read", "hooks": [{"type": "command", "command": "$INTERCEPT_CMD"}]}
    ]
  }
}
SETTINGS
        fi
    fi

    # Cursor: ~/.cursor/
    if [ -d "$HOME/.cursor" ]; then
        DETECTED="${DETECTED}Cursor, "
    fi

    # Windsurf: ~/.codeium/windsurf/
    if [ -d "$HOME/.codeium/windsurf" ]; then
        DETECTED="${DETECTED}Windsurf, "
    fi

    # Remove trailing ", "
    DETECTED="${DETECTED%, }"

    if [ -n "$DETECTED" ]; then
        ok "Guard activated for: ${BOLD}${DETECTED}${RESET}"
        ok "25 rules protecting against destructive actions"
    else
        ok "Guard rules installed (25 rules)"
        info "Run ${BOLD}savants mcp install${RESET} to connect to your AI editor"
    fi
}

ensure_path() {
    PATH_ADDED=""
    PATH_MANUAL=""
    case ":$PATH:" in
        *":$BIN_DIR:"*) export PATH="$BIN_DIR:$PATH"; return ;;
    esac
    SHELL_NAME="$(basename "$SHELL" 2>/dev/null || echo "bash")"
    case "$SHELL_NAME" in
        zsh)  RC="$HOME/.zshrc" ;;
        fish) RC="$HOME/.config/fish/config.fish" ;;
        *)    RC="$HOME/.bashrc" ;;
    esac
    if [ -f "$RC" ] && [ -w "$RC" ] && ! grep -q "savants/bin" "$RC" 2>/dev/null; then
        printf '\n# Savants\nexport PATH="%s:$PATH"\n' "$BIN_DIR" >> "$RC"
        PATH_ADDED="$RC"
    elif ! echo "$PATH" | grep -q "savants/bin"; then
        PATH_MANUAL="1"
    fi
    export PATH="$BIN_DIR:$PATH"
}

# Print PATH instructions at the very end (after all other output)
print_path_notice() {
    if [ -n "$PATH_ADDED" ]; then
        printf "  %sAdded to PATH in %s%s\n" "$DIM" "$PATH_ADDED" "$RESET"
        printf "  %sRestart your shell or run: %ssource %s%s\n\n" "$DIM" "$BOLD" "$PATH_ADDED" "$RESET"
    elif [ -n "$PATH_MANUAL" ]; then
        printf "  %s────────────────────────────────────────────%s\n" "$DIM" "$RESET"
        printf "  %sAdd this to your shell config (%s.bashrc%s / %s.zshrc%s):%s\n\n" "$DIM" "$BOLD" "$RESET$DIM" "$BOLD" "$RESET$DIM" "$RESET"
        printf "    %sexport PATH=\"%s:\$PATH\"%s\n\n" "$CYAN" "$BIN_DIR" "$RESET"
        printf "  %sThen restart your shell or run:%s\n" "$DIM" "$RESET"
        printf "    %ssource ~/.bashrc%s  %s# or ~/.zshrc%s\n\n" "$CYAN" "$RESET" "$DIM" "$RESET"
    fi
}

# ─── Anonymous install telemetry (respects DO_NOT_TRACK) ────────────────────
install_telemetry() {
    if [ -n "${DO_NOT_TRACK:-}" ] || [ -n "${SAVANTS_DO_NOT_TRACK:-}" ]; then
        return
    fi

    TELEM_ID=""
    if [ -f "$SAVANTS_HOME/state.json" ] && command -v python3 >/dev/null 2>&1; then
        TELEM_ID=$(python3 -c "import sys,json; print(json.load(open('$SAVANTS_HOME/state.json')).get('telemetry_id',''))" 2>/dev/null || true)
    fi

    if [ -z "$TELEM_ID" ]; then
        TELEM_ID="sv_$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n' | head -c 16)"
        # Save telemetry_id to state.json
        if [ -f "$SAVANTS_HOME/state.json" ] && command -v python3 >/dev/null 2>&1; then
            python3 -c "
import json
d = json.load(open('$SAVANTS_HOME/state.json'))
d['telemetry_id'] = '$TELEM_ID'
d['telemetry_enabled'] = True
json.dump(d, open('$SAVANTS_HOME/state.json', 'w'), indent=2)
" 2>/dev/null || true
        else
            mkdir -p "$SAVANTS_HOME"
            echo "{\"telemetry_id\":\"$TELEM_ID\",\"telemetry_enabled\":true}" > "$SAVANTS_HOME/state.json"
        fi
    fi

    # Fire and forget — don't block install on telemetry
    curl -s -X POST "https://api.savants.cloud/api/v1/telemetry" \
        -H "Content-Type: application/json" \
        -H "User-Agent: savants-installer" \
        -d "{\"telemetry_id\":\"$TELEM_ID\",\"event\":\"install\",\"version\":\"${INSTALLED_VERSION:-unknown}\",\"os\":\"$(uname -s)\",\"arch\":\"$(uname -m)\"}" \
        >/dev/null 2>&1 &
}

main "$@"

# Send install telemetry after main completes (non-blocking)
install_telemetry
