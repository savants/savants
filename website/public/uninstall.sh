#!/bin/sh
# Savants uninstaller — cleanly removes everything
#
# curl -fsSL https://releases.savants.dev/latest/uninstall.sh | sh
# Or: savants uninstall
#
# What gets removed:
#   1. ~/.savants/ directory (binary, config, guard rules, data)
#   2. Claude Code hooks referencing savants in ~/.claude/settings.json
#   3. PATH entry from .bashrc / .zshrc / config.fish

set -e

SAVANTS_HOME="${SAVANTS_HOME:-$HOME/.savants}"

# Colors
if [ -t 1 ]; then
    CYAN=$(printf '\033[36m'); GREEN=$(printf '\033[32m'); YELLOW=$(printf '\033[33m'); RED=$(printf '\033[31m')
    BOLD=$(printf '\033[1m'); DIM=$(printf '\033[2m'); RESET=$(printf '\033[0m')
else
    CYAN=''; GREEN=''; YELLOW=''; RED=''; BOLD=''; DIM=''; RESET=''
fi

info()  { printf "%s>%s %s\n" "$CYAN" "$RESET" "$*"; }
ok()    { printf "%s>%s %s\n" "$GREEN" "$RESET" "$*"; }
warn()  { printf "%s!%s %s\n" "$YELLOW" "$RESET" "$*"; }

echo ""
echo "${BOLD}  Savants Uninstaller${RESET}"
echo ""

# ─── Step 1: Remove Claude Code hooks ────────────────────

SETTINGS_FILE="$HOME/.claude/settings.json"
if [ -f "$SETTINGS_FILE" ] && command -v python3 >/dev/null 2>&1; then
    HOOK_COUNT=$(python3 -c "
import json, sys
try:
    d = json.load(open('$SETTINGS_FILE'))
    hooks = d.get('hooks', {})
    count = 0
    for event in list(hooks.keys()):
        hooks[event] = [h for h in hooks[event] if 'savants' not in json.dumps(h)]
        if not hooks[event]:
            del hooks[event]
        else:
            count += len([h for h in hooks.get(event, []) if 'savants' in json.dumps(h)])
    # Count removed hooks
    original = json.load(open('$SETTINGS_FILE'))
    orig_count = sum(1 for e in original.get('hooks',{}).values() for h in e if 'savants' in json.dumps(h))
    d['hooks'] = hooks
    json.dump(d, open('$SETTINGS_FILE', 'w'), indent=2)
    print(orig_count)
except Exception as e:
    print(0)
" 2>/dev/null)
    if [ "$HOOK_COUNT" -gt 0 ] 2>/dev/null; then
        ok "Removed $HOOK_COUNT Claude Code hooks from settings.json"
    else
        info "No Claude Code hooks to remove"
    fi
else
    info "No Claude Code settings found"
fi

# ─── Step 2: Remove PATH from shell config ───────────────

for RC_FILE in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.config/fish/config.fish"; do
    if [ -f "$RC_FILE" ] && grep -q "savants" "$RC_FILE" 2>/dev/null; then
        # Remove lines containing savants PATH
        if command -v sed >/dev/null 2>&1; then
            sed -i.bak '/savants/d' "$RC_FILE"
            rm -f "${RC_FILE}.bak"
            ok "Removed PATH entry from $(basename "$RC_FILE")"
        else
            warn "Found savants in $RC_FILE — remove manually"
        fi
    fi
done

# ─── Step 3: Stop running processes ──────────────────────

if command -v pgrep >/dev/null 2>&1; then
    PIDS=$(pgrep -f "savants serve\|savants daemon\|savants agent" 2>/dev/null || true)
    if [ -n "$PIDS" ]; then
        kill $PIDS 2>/dev/null || true
        ok "Stopped running savants processes"
    fi
fi

# ─── Step 4: Remove ~/.savants directory ─────────────────

if [ -d "$SAVANTS_HOME" ]; then
    # Show what's being removed
    SIZE=$(du -sh "$SAVANTS_HOME" 2>/dev/null | cut -f1)
    FILE_COUNT=$(find "$SAVANTS_HOME" -type f 2>/dev/null | wc -l)

    rm -rf "$SAVANTS_HOME"
    ok "Removed $SAVANTS_HOME ($FILE_COUNT files, $SIZE)"
else
    info "No ~/.savants directory found"
fi

# ─── Step 5: Remove Windows-specific (if applicable) ─────

if [ -n "$USERPROFILE" ] && [ -d "$USERPROFILE/.savants" ]; then
    rm -rf "$USERPROFILE/.savants"
    ok "Removed Windows savants directory"
fi

# ─── Done ─────────────────────────────────────────────────

echo ""
echo "${GREEN}${BOLD}  Savants uninstalled.${RESET}"
echo ""
echo "  ${DIM}What was removed:${RESET}"
echo "    ${DIM}~/.savants/ — binary, config, guard rules, data${RESET}"
echo "    ${DIM}Claude Code hooks — PreToolUse + PostToolUse${RESET}"
echo "    ${DIM}PATH entries — .bashrc / .zshrc${RESET}"
echo ""
echo "  ${DIM}To reinstall: curl -fsSL savants.sh | sh${RESET}"
echo ""
