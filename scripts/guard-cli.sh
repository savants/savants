#!/usr/bin/env bash
# savants guard — composable guardrails for AI coding agents
# Usage:
#   savants guard preset standard+secrets+git-safe
#   savants guard add "when tool eq 'Bash' and command contains 'rm' then block"
#   savants guard list
#   savants guard stats
#   savants guard reset

set -euo pipefail

SAVANTS_DIR="${HOME}/.savants"
RULES_FILE="${SAVANTS_DIR}/guard-rules.json"
STATS_FILE="${SAVANTS_DIR}/hook-stats.jsonl"
LOCK_FILE="${SAVANTS_DIR}/profiles.lock"
PROFILES_DIR="$(dirname "$0")/../packages/guard-profiles/presets"
CLOUD_API="${SAVANTS_CLOUD_API:-https://api.savants.cloud/api/v1/profiles}"

# If installed globally, profiles are bundled
if [ ! -d "$PROFILES_DIR" ]; then
  PROFILES_DIR="${SAVANTS_DIR}/profiles"
fi

cmd="${1:-help}"
shift || true

case "$cmd" in
  install)
    # Install a guard profile from cloud, GitHub, or URL
    PROFILE_NAME="${1:-}"
    if [ -z "$PROFILE_NAME" ]; then
      echo "Usage: savants guard install <source>"
      echo ""
      echo "Sources:"
      echo "  @user/name          Cloud profile (latest version)"
      echo "  @user/name@1.2.0    Cloud profile (exact version)"
      echo "  @user/name@^1       Cloud profile (semver range)"
      echo "  nixos-safe           Community profile from GitHub"
      echo "  https://...          Raw URL to a JSON rules file"
      echo ""
      echo "Examples:"
      echo "  savants guard install @miguel/nixos-flake-only"
      echo "  savants guard install @miguel/nixos-flake-only@^1"
      echo "  savants guard install nixos-safe"
      echo "  savants guard install https://example.com/rules.json"
      exit 1
    fi

    CUSTOM_DIR="${SAVANTS_DIR}/custom-profiles"
    mkdir -p "$CUSTOM_DIR"

    if [[ "$PROFILE_NAME" == @* ]]; then
      # Cloud profile: @owner/name or @owner/name@version
      HANDLE="${PROFILE_NAME#@}"

      # Split on @ to get version specifier
      if [[ "$HANDLE" == *@* ]]; then
        VERSION_SPEC="${HANDLE##*@}"
        HANDLE="${HANDLE%@*}"
      else
        VERSION_SPEC=""
      fi

      OWNER="${HANDLE%%/*}"
      NAME="${HANDLE#*/}"

      if [ -z "$OWNER" ] || [ -z "$NAME" ]; then
        echo "Invalid handle. Use: @owner/name"
        exit 1
      fi

      echo "Installing @${OWNER}/${NAME}..."

      if [ -n "$VERSION_SPEC" ]; then
        API_URL="${CLOUD_API}/${OWNER}/${NAME}/${VERSION_SPEC}"
      else
        API_URL="${CLOUD_API}/${OWNER}/${NAME}"
      fi

      RESPONSE=$(curl -fsSL "$API_URL" 2>/dev/null)
      if [ $? -ne 0 ]; then
        echo "  Profile @${OWNER}/${NAME} not found on savants.cloud"
        exit 1
      fi

      # Extract rules and version from response
      INSTALLED_VERSION=$(echo "$RESPONSE" | python3 -c "import json,sys; print(json.load(sys.stdin)['version'])" 2>/dev/null)
      RULES=$(echo "$RESPONSE" | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['rules']))" 2>/dev/null)

      if [ -z "$INSTALLED_VERSION" ] || [ -z "$RULES" ]; then
        echo "  Error: invalid response from cloud API"
        exit 1
      fi

      DEST="${CUSTOM_DIR}/${NAME}.json"

      # Read previous version from lock file for rollback support
      PREV_VERSION=""
      if [ -f "$LOCK_FILE" ]; then
        PREV_VERSION=$(python3 -c "
import json
lock = json.load(open('${LOCK_FILE}'))
entry = lock.get('@${OWNER}/${NAME}', {})
print(entry.get('version', ''))
" 2>/dev/null)
      fi

      echo "$RULES" > "$DEST"
      RULE_COUNT=$(python3 -c "import json; print(len(json.load(open('$DEST'))))" 2>/dev/null)
      echo "  Installed: @${OWNER}/${NAME}@${INSTALLED_VERSION} (${RULE_COUNT} rules)"

      # Update lock file
      python3 -c "
import json, os
lock_path = '${LOCK_FILE}'
try:
    lock = json.load(open(lock_path))
except:
    lock = {}
lock['@${OWNER}/${NAME}'] = {
    'version': '${INSTALLED_VERSION}',
    'pinned': '${VERSION_SPEC}' if '${VERSION_SPEC}' else '${INSTALLED_VERSION}',
    'installed': '$(date -u +%Y-%m-%d)',
    'previous': '${PREV_VERSION}' if '${PREV_VERSION}' else None
}
# Remove None values
entry = lock['@${OWNER}/${NAME}']
lock['@${OWNER}/${NAME}'] = {k: v for k, v in entry.items() if v is not None}
json.dump(lock, open(lock_path, 'w'), indent=2)
" 2>/dev/null

      # Notify cloud of install (fire and forget)
      curl -fsS -X POST "${CLOUD_API}/${OWNER}/${NAME}/install" \
        -H "Content-Type: application/json" \
        -d "{\"version\":\"${INSTALLED_VERSION}\"}" >/dev/null 2>&1 &

      echo ""
      echo "  Activate: savants guard preset standard+${NAME}"
      echo "  View:     cat $DEST"

    elif [[ "$PROFILE_NAME" == https://* ]] || [[ "$PROFILE_NAME" == http://* ]]; then
      # Raw URL install
      URL_NAME=$(basename "$PROFILE_NAME" .json)
      DEST="${CUSTOM_DIR}/${URL_NAME}.json"

      echo "Installing from URL..."
      if curl -fsSL "$PROFILE_NAME" -o "$DEST" 2>/dev/null; then
        if python3 -c "import json; rules=json.load(open('$DEST')); print(f'  Installed: {len(rules)} rules')" 2>/dev/null; then
          echo ""
          echo "  Activate: savants guard preset standard+${URL_NAME}"
          echo "  View:     cat $DEST"
        else
          echo "  Error: downloaded file is not valid JSON"
          rm -f "$DEST"
          exit 1
        fi
      else
        echo "  Failed to download from URL"
        exit 1
      fi

    else
      # Community profile from GitHub (original behavior)
      DEST="${CUSTOM_DIR}/${PROFILE_NAME}.json"
      URL="https://raw.githubusercontent.com/savants/savants/main/packages/guard-profiles/community/${PROFILE_NAME}.json"

      echo "Installing ${PROFILE_NAME}..."
      if curl -fsSL "$URL" -o "$DEST" 2>/dev/null; then
        if python3 -c "import json; rules=json.load(open('$DEST')); print(f'  Installed: {len(rules)} rules')" 2>/dev/null; then
          echo ""
          echo "  Activate: savants guard preset standard+${PROFILE_NAME}"
          echo "  View:     cat $DEST"
        else
          echo "  Error: downloaded file is not valid JSON"
          rm -f "$DEST"
          exit 1
        fi
      else
        echo "  Profile '${PROFILE_NAME}' not found in community registry."
        echo ""
        echo "  Browse available: https://github.com/savants/savants/tree/main/packages/guard-profiles/community"
        echo "  Create your own:  ~/.savants/custom-profiles/${PROFILE_NAME}.json"
        exit 1
      fi
    fi
    ;;

  publish)
    # Legacy alias for share
    exec "$0" share "$@"
    ;;

  share)
    # Share a guard profile to savants.cloud
    PROFILE_NAME="${1:-}"
    VERSION="${3:-1.0.0}"  # default version

    # Parse --version flag
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --version) VERSION="$2"; shift 2 ;;
        --description) DESCRIPTION="$2"; shift 2 ;;
        *) if [ -z "$PROFILE_NAME" ]; then PROFILE_NAME="$1"; fi; shift ;;
      esac
    done

    if [ -z "$PROFILE_NAME" ]; then
      echo "Usage: savants guard share <profile-name> [--version 1.0.0] [--description \"...\"]"
      echo ""
      echo "Publishes ~/.savants/custom-profiles/<name>.json to savants.cloud"
      echo ""
      echo "Examples:"
      echo "  savants guard share my-rules --version 1.0.0"
      echo "  savants guard share nixos-flake-only --version 1.1.0 --description \"NixOS flake safety\""
      exit 1
    fi

    CUSTOM_DIR="${SAVANTS_DIR}/custom-profiles"
    SOURCE="${CUSTOM_DIR}/${PROFILE_NAME}.json"
    if [ ! -f "$SOURCE" ]; then
      echo "Profile not found: $SOURCE"
      echo ""
      echo "Create it first:"
      echo "  mkdir -p $CUSTOM_DIR"
      echo "  echo '[\"when tool eq ...\" ]' > $SOURCE"
      exit 1
    fi

    # Get auth token
    API_KEY="${SAVANTS_API_KEY:-}"
    if [ -z "$API_KEY" ]; then
      STATE_FILE="${SAVANTS_DIR}/state.json"
      if [ -f "$STATE_FILE" ]; then
        API_KEY=$(python3 -c "import json; print(json.load(open('$STATE_FILE')).get('cloud_token',''))" 2>/dev/null)
      fi
    fi
    if [ -z "$API_KEY" ]; then
      echo "Not authenticated. Run: savants connect"
      exit 1
    fi

    # Build payload
    PAYLOAD=$(python3 -c "
import json
rules = json.load(open('${SOURCE}'))
payload = {
    'name': '${PROFILE_NAME}',
    'version': '${VERSION}',
    'rules': rules,
}
desc = '${DESCRIPTION:-}'
if desc:
    payload['description'] = desc
print(json.dumps(payload))
" 2>/dev/null)

    echo "Publishing ${PROFILE_NAME}@${VERSION}..."
    RESPONSE=$(curl -fsS -X POST "${CLOUD_API}/publish" \
      -H "Authorization: Bearer ${API_KEY}" \
      -H "Content-Type: application/json" \
      -d "$PAYLOAD" 2>&1)

    if [ $? -eq 0 ]; then
      HANDLE=$(echo "$RESPONSE" | python3 -c "import json,sys; print(json.load(sys.stdin).get('handle',''))" 2>/dev/null)
      echo "  Shared! Install with: savants guard install ${HANDLE}"
    else
      echo "  Error: $RESPONSE"
      exit 1
    fi
    ;;

  rollback)
    # Rollback a cloud-installed profile to its previous version
    PROFILE_NAME="${1:-}"
    if [ -z "$PROFILE_NAME" ]; then
      echo "Usage: savants guard rollback @owner/name"
      echo ""
      echo "Restores the previously installed version from profiles.lock"
      exit 1
    fi

    # Normalize handle
    HANDLE="$PROFILE_NAME"
    if [[ "$HANDLE" != @* ]]; then
      HANDLE="@${HANDLE}"
    fi

    if [ ! -f "$LOCK_FILE" ]; then
      echo "No profiles.lock found. Nothing to rollback."
      exit 1
    fi

    python3 -c "
import json, sys, subprocess

lock = json.load(open('${LOCK_FILE}'))
entry = lock.get('${HANDLE}')
if not entry:
    print(f'  ${HANDLE} not found in profiles.lock')
    sys.exit(1)

prev = entry.get('previous')
if not prev:
    print(f'  No previous version recorded for ${HANDLE}')
    sys.exit(1)

current = entry.get('version', '?')
print(f'  Rolling back ${HANDLE} from {current} to {prev}...')
" 2>/dev/null || exit 1

    PREV_VERSION=$(python3 -c "
import json
lock = json.load(open('${LOCK_FILE}'))
print(lock['${HANDLE}']['previous'])
" 2>/dev/null)

    # Extract owner/name from handle
    CLEAN_HANDLE="${HANDLE#@}"
    OWNER="${CLEAN_HANDLE%%/*}"
    NAME="${CLEAN_HANDLE#*/}"

    # Re-install the previous version
    exec "$0" install "@${OWNER}/${NAME}@${PREV_VERSION}"
    ;;

  versions)
    # List all versions of a cloud profile
    PROFILE_NAME="${1:-}"
    if [ -z "$PROFILE_NAME" ]; then
      echo "Usage: savants guard versions @owner/name"
      exit 1
    fi

    # Normalize handle
    HANDLE="${PROFILE_NAME#@}"
    OWNER="${HANDLE%%/*}"
    NAME="${HANDLE#*/}"

    if [ -z "$OWNER" ] || [ -z "$NAME" ]; then
      echo "Invalid handle. Use: @owner/name"
      exit 1
    fi

    RESPONSE=$(curl -fsSL "${CLOUD_API}/${OWNER}/${NAME}/versions" 2>/dev/null)
    if [ $? -ne 0 ]; then
      echo "  Profile @${OWNER}/${NAME} not found"
      exit 1
    fi

    # Get current installed version from lock file
    CURRENT=""
    if [ -f "$LOCK_FILE" ]; then
      CURRENT=$(python3 -c "
import json
lock = json.load(open('${LOCK_FILE}'))
print(lock.get('@${OWNER}/${NAME}', {}).get('version', ''))
" 2>/dev/null)
    fi

    echo "$RESPONSE" | python3 -c "
import json, sys
data = json.load(sys.stdin)
current = '${CURRENT}'
print(f'Versions of {data[\"handle\"]}:')
print()
for v in data['versions']:
    marker = ' (installed)' if v['version'] == current else ''
    print(f'  {v[\"version\"]:>10}  {v[\"rule_count\"]:>3} rules  {v[\"installs\"]:>4} installs  {v[\"created_at\"]}{marker}')
" 2>/dev/null
    ;;

  preset)
    # Parse profile names: standard+secrets+git-safe
    PRESET_STR="${1:-standard}"
    IFS='+' read -ra PROFILES <<< "$PRESET_STR"

    ALL_RULES="[]"
    LOADED=""

    CUSTOM_DIR="${SAVANTS_DIR}/custom-profiles"

    for profile in "${PROFILES[@]}"; do
      # Check built-in profiles first, then custom user profiles
      PROFILE_FILE="${PROFILES_DIR}/${profile}.json"
      if [ ! -f "$PROFILE_FILE" ] && [ -f "${CUSTOM_DIR}/${profile}.json" ]; then
        PROFILE_FILE="${CUSTOM_DIR}/${profile}.json"
      fi
      if [ ! -f "$PROFILE_FILE" ]; then
        echo "Unknown profile: ${profile}"
        echo "Built-in: minimal, standard, paranoid, comprehensive, battle-tested, nixos-safe,"
        echo "  filesystem-safe, credentials-safe, git-safe, database-safe, k8s-safe,"
        echo "  cloud-safe, network-safe, publish-safe, system-safe, cicd-safe, persistence-safe"
        echo ""
        echo "Custom profiles: place JSON files in ~/.savants/custom-profiles/"
        echo "  system-safe, cicd-safe, persistence-safe, secrets, infra-safe"
        exit 1
      fi

      # Merge rules (deduplicate)
      ALL_RULES=$(python3 -c "
import json, sys
existing = json.loads(sys.argv[1])
new = json.load(open(sys.argv[2]))
for r in new:
    if r not in existing:
        existing.append(r)
print(json.dumps(existing, indent=2))
" "$ALL_RULES" "$PROFILE_FILE")

      COUNT=$(python3 -c "import json; print(len(json.load(open('$PROFILE_FILE'))))")
      LOADED="${LOADED}  ✓ ${profile} (${COUNT} rules)\n"
    done

    mkdir -p "$SAVANTS_DIR"
    echo "$ALL_RULES" > "$RULES_FILE"

    # Track which preset was activated
    python3 -c "
import json
state = {'preset': '${PRESET_STR}', 'profiles': '${PRESET_STR}'.split('+')}
json.dump(state, open('${SAVANTS_DIR}/guard-state.json', 'w'), indent=2)
" 2>/dev/null

    TOTAL=$(echo "$ALL_RULES" | python3 -c "import json,sys; print(len(json.load(sys.stdin)))")
    echo "Guard profiles activated:"
    echo -e "$LOADED"
    echo "Total: ${TOTAL} rules → ${RULES_FILE}"
    echo ""
    echo "Your AI coding agent is now protected."
    echo "Use --dangerously-skip-permissions with confidence."
    ;;

  add)
    RULE="$*"
    if [ -z "$RULE" ]; then
      echo "Usage: savants guard add \"when tool eq 'Bash' and command contains 'rm' then block\""
      exit 1
    fi

    if [ ! -f "$RULES_FILE" ]; then
      echo "[]" > "$RULES_FILE"
    fi

    python3 -c "
import json
rules = json.load(open('${RULES_FILE}'))
rule = '''${RULE}'''
if rule not in rules:
    rules.append(rule)
    json.dump(rules, open('${RULES_FILE}', 'w'), indent=2)
    print(f'Added: {rule}')
    print(f'Total: {len(rules)} rules active')
else:
    print('Rule already exists')
"
    ;;

  remove)
    RULE="$*"
    python3 -c "
import json
rules = json.load(open('${RULES_FILE}'))
rule = '''${RULE}'''
if rule in rules:
    rules.remove(rule)
    json.dump(rules, open('${RULES_FILE}', 'w'), indent=2)
    print(f'Removed: {rule}')
    print(f'Total: {len(rules)} rules active')
else:
    print('Rule not found')
"
    ;;

  list)
    if [ ! -f "$RULES_FILE" ]; then
      echo "No guard rules active. Run: savants guard preset standard"
      exit 0
    fi

    # Show pause status if paused
    PAUSE_FILE="${SAVANTS_DIR}/guard-paused"
    if [ -f "$PAUSE_FILE" ]; then
      CONTENT=$(cat "$PAUSE_FILE" 2>/dev/null)
      if [ -n "$CONTENT" ]; then
        echo "  PAUSED (resumes at $CONTENT)"
      else
        echo "  PAUSED (indefinitely) — run: savants guard on"
      fi
      echo ""
    fi

    python3 -c "
import json
rules = json.load(open('${RULES_FILE}'))
print(f'{len(rules)} active guard rules:')
print()
for i, r in enumerate(rules, 1):
    action = 'block' if 'then block' in r else 'require_approval' if 'require_approval' in r else 'other'
    icon = 'x' if action == 'block' else '!'
    print(f'  {i:>2}. [{icon}] {r}')
print()
print('Disable a rule: savants guard disable <number>')
"
    ;;

  stats)
    if [ ! -f "$STATS_FILE" ]; then
      echo "No guard events yet. Use Claude Code with savants guard enabled."
      exit 0
    fi
    python3 -c "
import json
events = []
for line in open('${STATS_FILE}'):
    try: events.append(json.loads(line.strip()))
    except: pass

blocks = [e for e in events if e.get('action') == 'block' and e.get('reason') == 'guard_rule']
allows = [e for e in events if e.get('action') == 'allow']
total = len(events)

print('Guard Statistics')
print('=' * 40)
print(f'Total intercepted:  {total}')
print(f'Blocked by guard:   {len(blocks)}')
print(f'Allowed:            {len(allows)}')
print()

if blocks:
    print('Recent blocks:')
    for b in blocks[-5:]:
        print(f'  🛑 {b.get(\"tool\", \"?\")} — {b.get(\"detail\", \"\")}')

print()
import os
rf = os.path.expanduser('~/.savants/guard-rules.json')
try: rules = json.load(open(rf))
except: rules = []
print(f'Active rules:       {len(rules)}')
print(f'Blocks prevented:   {len(blocks)}')
if blocks:
    print()
    print(f'  Your guardrails prevented {len(blocks)} potentially dangerous actions.')
print()
print('--- Upgrade to Pro ---')
print('  See what your TEAM blocked:        savants.cloud/dashboard/guard-log')
print('  Update rules without restarting:   savants.cloud/dashboard/guard-rules')
print('  Share rules across all developers: managed mode')
"
    ;;

  sync)
    SYNC_CMD="${1:-}"
    shift || true

    # Resolve API key for all sync subcommands
    SYNC_API_KEY="${SAVANTS_API_KEY:-}"
    if [ -z "$SYNC_API_KEY" ]; then
      STATE_FILE="${SAVANTS_DIR}/state.json"
      if [ -f "$STATE_FILE" ]; then
        SYNC_API_KEY=$(python3 -c "import json; print(json.load(open('$STATE_FILE')).get('cloud_token',''))" 2>/dev/null)
      fi
    fi
    SYNC_FILE="${SAVANTS_DIR}/guard-sync.json"
    CLOUD_GUARD_API="${SAVANTS_CLOUD_GUARD_API:-https://api.savants.cloud/api/v1/guard}"

    case "$SYNC_CMD" in
      push)
        if [ -z "$SYNC_API_KEY" ]; then
          echo "Not authenticated. Run: savants connect"
          exit 1
        fi

        if [ ! -f "$RULES_FILE" ]; then
          echo "No guard rules to push. Run: savants guard preset standard"
          exit 1
        fi

        # Determine preset from guard-state.json if available
        GUARD_STATE_FILE="${SAVANTS_DIR}/guard-state.json"
        PRESET=""
        if [ -f "$GUARD_STATE_FILE" ]; then
          PRESET=$(python3 -c "import json; print(json.load(open('$GUARD_STATE_FILE')).get('preset',''))" 2>/dev/null)
        fi

        MACHINE_ID=$(hostname 2>/dev/null || echo "unknown")

        python3 -c "
import json, sys
try:
    from urllib.request import Request, urlopen
except:
    print('Python urllib required')
    sys.exit(1)

rules = json.load(open('${RULES_FILE}'))
payload = {
    'rules': rules,
    'preset': '${PRESET}' or None,
    'custom_rules': [],
    'machine_id': '${MACHINE_ID}',
}

data = json.dumps(payload).encode()
req = Request(
    '${CLOUD_GUARD_API}/config',
    data=data,
    headers={
        'Authorization': 'Bearer ${SYNC_API_KEY}',
        'Content-Type': 'application/json',
        'User-Agent': 'savants-cli/0.21.0',
    },
    method='POST',
)
try:
    resp = urlopen(req, timeout=10)
    result = json.loads(resp.read().decode())
    version = result.get('version', '?')
    count = result.get('rules_count', len(rules))
    print(f'Config synced to cloud ({count} rules, version {version})')

    # Update local sync state
    import os
    from datetime import datetime, timezone
    sync_state = {}
    sync_path = '${SYNC_FILE}'
    if os.path.exists(sync_path):
        try: sync_state = json.load(open(sync_path))
        except: pass
    sync_state['local_version'] = version
    sync_state['cloud_version'] = version
    sync_state['last_push'] = datetime.now(timezone.utc).isoformat()
    sync_state['machine_id'] = '${MACHINE_ID}'
    json.dump(sync_state, open(sync_path, 'w'), indent=2)
except Exception as e:
    print(f'Sync error: {e}')
    sys.exit(1)
"
        ;;

      pull)
        if [ -z "$SYNC_API_KEY" ]; then
          echo "Not authenticated. Run: savants connect"
          exit 1
        fi

        python3 -c "
import json, sys
try:
    from urllib.request import Request, urlopen
except:
    print('Python urllib required')
    sys.exit(1)

req = Request(
    '${CLOUD_GUARD_API}/config',
    headers={
        'Authorization': 'Bearer ${SYNC_API_KEY}',
        'User-Agent': 'savants-cli/0.21.0',
    },
)
try:
    resp = urlopen(req, timeout=10)
    result = json.loads(resp.read().decode())
    rules = result.get('rules', [])
    version = result.get('version', '?')

    json.dump(rules, open('${RULES_FILE}', 'w'), indent=2)
    print(f'Pulled config from cloud ({len(rules)} rules, version {version})')

    # Update local sync state
    import os
    from datetime import datetime, timezone
    sync_state = {}
    sync_path = '${SYNC_FILE}'
    if os.path.exists(sync_path):
        try: sync_state = json.load(open(sync_path))
        except: pass
    sync_state['local_version'] = version
    sync_state['cloud_version'] = version
    sync_state['last_check'] = datetime.now(timezone.utc).isoformat()
    sync_state['machine_id'] = '$(hostname 2>/dev/null || echo unknown)'
    json.dump(sync_state, open(sync_path, 'w'), indent=2)

    # Update guard-state.json with preset if returned
    preset = result.get('preset', '')
    if preset:
        state_path = '${SAVANTS_DIR}/guard-state.json'
        guard_state = {}
        if os.path.exists(state_path):
            try: guard_state = json.load(open(state_path))
            except: pass
        guard_state['preset'] = preset
        json.dump(guard_state, open(state_path, 'w'), indent=2)
except Exception as e:
    print(f'Pull error: {e}')
    sys.exit(1)
"
        ;;

      status)
        python3 -c "
import json, sys, os
from datetime import datetime, timezone

sync_path = '${SYNC_FILE}'
rules_path = '${RULES_FILE}'

# Load sync state
sync_state = {}
if os.path.exists(sync_path):
    try: sync_state = json.load(open(sync_path))
    except: pass

# Load local rules
local_rules = []
if os.path.exists(rules_path):
    try: local_rules = json.load(open(rules_path))
    except: pass

local_version = sync_state.get('local_version', 0)
cloud_version = sync_state.get('cloud_version', 0)
auto_sync = sync_state.get('enabled', False)
last_check = sync_state.get('last_check', '')
last_push = sync_state.get('last_push', '')

def relative_time(iso_str):
    if not iso_str:
        return 'never'
    try:
        dt = datetime.fromisoformat(iso_str.replace('Z', '+00:00'))
        now = datetime.now(timezone.utc)
        delta = now - dt
        secs = int(delta.total_seconds())
        if secs < 60: return f'{secs}s ago'
        if secs < 3600: return f'{secs // 60}m ago'
        if secs < 86400: return f'{secs // 3600}h ago'
        return f'{secs // 86400}d ago'
    except:
        return iso_str

print('Guard Sync Status')
print(f'  Local:  {len(local_rules)} rules (version {local_version}, updated {relative_time(last_push)})')
print(f'  Cloud:  version {cloud_version} (checked {relative_time(last_check)})')

if local_version == cloud_version and cloud_version > 0:
    print(f'  Status: IN SYNC')
elif cloud_version > local_version:
    print(f\"  Status: OUT OF SYNC — run 'savants guard sync pull' to update\")
elif local_version > cloud_version and cloud_version > 0:
    print(f\"  Status: LOCAL AHEAD — run 'savants guard sync push' to upload\")
elif cloud_version == 0:
    print(f\"  Status: NOT SYNCED — run 'savants guard sync push' to start\")

auto_str = 'on (checks every 5 min)' if auto_sync else 'off'
print(f'  Auto-sync: {auto_str}')
"
        ;;

      auto)
        AUTO_ACTION="${1:-}"
        case "$AUTO_ACTION" in
          on)
            python3 -c "
import json, os
from datetime import datetime, timezone
sync_path = '${SYNC_FILE}'
sync_state = {}
if os.path.exists(sync_path):
    try: sync_state = json.load(open(sync_path))
    except: pass
sync_state['enabled'] = True
json.dump(sync_state, open(sync_path, 'w'), indent=2)
print('Auto-sync enabled. Guard config will sync every 5 minutes.')
"
            ;;
          off)
            python3 -c "
import json, os
sync_path = '${SYNC_FILE}'
sync_state = {}
if os.path.exists(sync_path):
    try: sync_state = json.load(open(sync_path))
    except: pass
sync_state['enabled'] = False
json.dump(sync_state, open(sync_path, 'w'), indent=2)
print('Auto-sync disabled.')
"
            ;;
          *)
            echo "Usage: savants guard sync auto on|off"
            ;;
        esac
        ;;

      events)
        # Legacy: sync events to cloud (original sync behavior)
        if [ -z "$SYNC_API_KEY" ]; then
          echo "No API key found. Set SAVANTS_API_KEY or run savants connect."
          echo "  Sign up: savants.cloud/activate"
          exit 1
        fi

        if [ ! -f "$STATS_FILE" ]; then
          echo "No local events to sync."
          exit 0
        fi

        SYNC_MARKER="${SAVANTS_DIR}/guard-sync-offset"
        OFFSET=$(cat "$SYNC_MARKER" 2>/dev/null || echo "0")

        python3 -c "
import json, sys
try:
    from urllib.request import Request, urlopen
except:
    print('Python urllib required')
    sys.exit(1)

events = []
offset = int('${OFFSET}')
with open('${STATS_FILE}') as f:
    for i, line in enumerate(f):
        if i < offset:
            continue
        try:
            e = json.loads(line.strip())
            events.append({
                'context_hash': str(hash(e.get('detail',''))),
                'action': e.get('detail','')[:120] if e.get('action') == 'block' else '',
                'tool': e.get('tool',''),
                'result': 'blocked' if e.get('action') in ('block','suggest','rewrite','ask') else 'allowed',
                'matched_rule': e.get('detail','')[:200] if e.get('reason') == 'guard_rule' else None,
                'timestamp': e.get('ts',''),
            })
        except:
            pass

if not events:
    print('No new events to sync.')
    sys.exit(0)

# Batch in groups of 200
total_sent = 0
for i in range(0, len(events), 200):
    batch = events[i:i+200]
    payload = json.dumps({'events': batch}).encode()
    req = Request(
        'https://api.savants.cloud/api/v1/guard/events',
        data=payload,
        headers={
            'Authorization': 'Bearer ${SYNC_API_KEY}',
            'Content-Type': 'application/json',
            'User-Agent': 'savants-cli/0.21.0',
        },
        method='POST',
    )
    try:
        resp = urlopen(req, timeout=10)
        if resp.status in (200, 202):
            total_sent += len(batch)
    except Exception as e:
        print(f'Sync error: {e}')
        break

# Update offset
with open('${SYNC_MARKER}', 'w') as f:
    f.write(str(offset + total_sent))

print(f'Synced {total_sent} events to Savants Cloud.')
print(f'  View: savants.cloud/dashboard/guard-analytics')
"
        ;;

      *)
        echo "Usage: savants guard sync <command>"
        echo ""
        echo "Commands:"
        echo "  push          Push local guard config to cloud"
        echo "  pull          Pull guard config from cloud"
        echo "  status        Show local vs cloud sync status"
        echo "  auto on|off   Enable/disable automatic sync checking"
        echo "  events        Sync guard events (blocks/allows) to cloud"
        ;;
    esac
    ;;

  off)
    DURATION="${1:-}"
    PAUSE_FILE="${SAVANTS_DIR}/guard-paused"

    if [ -n "$DURATION" ]; then
      # Parse duration: 10m, 1h, 30s
      UNIT="${DURATION: -1}"
      NUM="${DURATION%?}"
      case "$UNIT" in
        s) SECONDS_ADD="$NUM" ;;
        m) SECONDS_ADD=$((NUM * 60)) ;;
        h) SECONDS_ADD=$((NUM * 3600)) ;;
        *) echo "Usage: savants guard off [10m|1h|30s]"; exit 1 ;;
      esac
      EXPIRY=$(python3 -c "
from datetime import datetime, timedelta, timezone
exp = datetime.now(timezone.utc) + timedelta(seconds=$SECONDS_ADD)
print(exp.isoformat())
")
      echo "$EXPIRY" > "$PAUSE_FILE"
      echo "Guard paused for ${DURATION}."
      echo "  Resumes automatically at $(date -d "+${SECONDS_ADD} seconds" '+%H:%M:%S' 2>/dev/null || echo "$EXPIRY")"
      echo "  Or manually: savants guard on"
    else
      touch "$PAUSE_FILE"
      echo "Guard paused (indefinitely)."
      echo "  Resume: savants guard on"
    fi
    ;;

  on)
    PAUSE_FILE="${SAVANTS_DIR}/guard-paused"
    rm -f "$PAUSE_FILE"
    if [ -f "$RULES_FILE" ]; then
      COUNT=$(python3 -c "import json; print(len(json.load(open('$RULES_FILE'))))")
      echo "Guard resumed. ${COUNT} rules active."
    else
      echo "Guard resumed (no rules loaded)."
      echo "  Run: savants guard preset standard"
    fi
    ;;

  status)
    PAUSE_FILE="${SAVANTS_DIR}/guard-paused"
    echo ""
    if [ -f "$PAUSE_FILE" ]; then
      CONTENT=$(cat "$PAUSE_FILE" 2>/dev/null)
      if [ -n "$CONTENT" ]; then
        echo "  Guard: PAUSED (resumes at $CONTENT)"
      else
        echo "  Guard: PAUSED (indefinitely)"
      fi
      echo "  Resume: savants guard on"
    elif [ -f "$RULES_FILE" ]; then
      python3 -c "
import json
rules = json.load(open('${RULES_FILE}'))
blocks = sum(1 for r in rules if 'then block' in r)
suggests = sum(1 for r in rules if 'then suggest' in r)
rewrites = sum(1 for r in rules if 'then rewrite' in r)
asks = sum(1 for r in rules if 'then ask' in r or 'then require_approval' in r)
print(f'  Guard: ACTIVE ({len(rules)} rules)')
print()
print(f'    {blocks} block (hard stop)')
print(f'    {suggests} suggest (alternative offered)')
print(f'    {rewrites} rewrite (silent command swap)')
print(f'    {asks} ask (requires approval)')
print()
print(f'  Profile: standard')
print(f'  Rules file: ${RULES_FILE}')
"
    else
      echo "  Guard: INACTIVE (no rules loaded)"
      echo "  Activate: savants guard preset standard"
    fi

    # Show stats if available
    if [ -f "$STATS_FILE" ]; then
      python3 -c "
import json
events = []
for line in open('${STATS_FILE}'):
    try: events.append(json.loads(line.strip()))
    except: pass
blocks = sum(1 for e in events if e.get('action') == 'block' and e.get('reason') == 'guard_rule')
allows = sum(1 for e in events if e.get('action') == 'allow')
suggests = sum(1 for e in events if e.get('action') == 'suggest')
rewrites = sum(1 for e in events if e.get('action') == 'rewrite')
asks = sum(1 for e in events if e.get('action') == 'ask')
print()
print(f'  Recent activity:')
print(f'    {len(events)} total events')
print(f'    {blocks} blocked, {suggests} suggested, {rewrites} rewritten, {asks} asked')
print(f'    {allows} allowed')

# Block rate
total_events = len(events)
if total_events > 0:
    block_pct = blocks / total_events * 100
    print(f'    Block rate: {block_pct:.1f}% ({blocks}/{total_events} events)')
    if blocks == 0 and total_events > 50:
        print(f'    Note: 0 blocks in {total_events} events. Your guard rules are active but')
        print(f'    no dangerous actions have been attempted. This is normal for safe workflows.')

# Last event timestamp
if events:
    last_ts = events[-1].get('ts', '')
    print(f'    Last event: {last_ts}')

# Top triggered rules (from block/suggest/rewrite/ask events)
triggered = [e for e in events if e.get('reason') == 'guard_rule' and e.get('detail')]
if triggered:
    from collections import Counter
    rule_counts = Counter(e.get('detail','') for e in triggered)
    top = rule_counts.most_common(3)
    if top:
        print()
        print(f'  Top triggered rules:')
        for rule, count in top:
            print(f'    {count}x  {rule}')

# Never-triggered rules audit
import os
rf = os.path.expanduser('~/.savants/guard-rules.json')
try:
    rules = json.load(open(rf))
except:
    rules = []

if rules:
    triggered_details = set(e.get('detail','') for e in events if e.get('reason') == 'guard_rule' and e.get('detail'))
    never_triggered = [r for r in rules if r not in triggered_details]
    if never_triggered:
        print(f'    Never triggered: {len(never_triggered)} rules')
        for r in never_triggered[:5]:
            print(f'      {r}')
        if len(never_triggered) > 5:
            print(f'      ... and {len(never_triggered) - 5} more (run savants guard list)')

# Show last few non-allow events with timestamps
non_allow = [e for e in events if e.get('action') != 'allow']
if non_allow:
    recent = non_allow[-3:]
    print()
    print(f'  Recent guard events:')
    for e in reversed(recent):
        ts = e.get('ts','?')[:19]
        action = e.get('action','?')
        detail = e.get('detail','')
        tool = e.get('tool','?')
        # For rewrite events, show before/after if detail contains the arrow
        if action == 'rewrite' and '→' in detail:
            print(f'    [{ts}] {action} {tool}: {detail}')
        elif action == 'rewrite' and '->' in detail:
            print(f'    [{ts}] {action} {tool}: {detail}')
        else:
            print(f'    [{ts}] {action} {tool}: {detail}')
" 2>/dev/null
    fi

    # Telemetry status
    STATE_FILE="${SAVANTS_DIR}/state.json"
    if [ -f "$STATE_FILE" ]; then
      python3 -c "
import json, os
state = json.load(open('${STATE_FILE}'))
enabled = state.get('telemetry_enabled', True)
tid = state.get('telemetry_id', '')
env_off = os.environ.get('DO_NOT_TRACK') == '1' or os.environ.get('SAVANTS_DO_NOT_TRACK') == '1'

print()
if env_off:
    print('  Telemetry: disabled (env var)')
elif enabled:
    print(f'  Telemetry: on (id: {tid[:12]}...)' if len(tid) > 12 else f'  Telemetry: on (id: {tid})')
else:
    print('  Telemetry: off')
print('    Manage: savants config telemetry [on|off|status]')
" 2>/dev/null
    fi

    # Cloud sync status
    if [ -f "$STATE_FILE" ]; then
      HAS_TOKEN=$(python3 -c "
import json
try:
    s = json.load(open('${STATE_FILE}'))
    t = s.get('cloud_token', '')
    print('yes' if t else 'no')
except:
    print('no')
" 2>/dev/null)
      if [ "$HAS_TOKEN" = "yes" ]; then
        echo "  Cloud sync: connected (savants guard sync to push events)"
      else
        echo "  Cloud sync: not configured (run savants connect for team features)"
      fi
    else
      echo "  Cloud sync: not configured (run savants connect for team features)"
    fi
    echo ""
    ;;

  disable)
    # Disable a specific rule by number or substring (moves to disabled-rules.json)
    RULE_ID="$*"
    if [ -z "$RULE_ID" ]; then
      echo "Usage:"
      echo "  savants guard disable 3          # disable rule #3 (see 'savants guard list')"
      echo "  savants guard disable 'rm -rf'   # disable rules matching 'rm -rf'"
      exit 1
    fi

    if [ ! -f "$RULES_FILE" ]; then
      echo "No rules loaded."
      exit 1
    fi

    DISABLED_FILE="${SAVANTS_DIR}/disabled-rules.json"

    python3 -c "
import json, sys
rules = json.load(open('${RULES_FILE}'))
target = '''${RULE_ID}'''

# Load existing disabled rules
try:
    disabled = json.load(open('${DISABLED_FILE}'))
except:
    disabled = []

removed = []

# Try as number first
try:
    idx = int(target) - 1
    if 0 <= idx < len(rules):
        removed = [rules.pop(idx)]
    else:
        print(f'Rule #{idx+1} not found. You have {len(rules)} rules.')
        sys.exit(1)
except ValueError:
    # Try as substring match
    removed = [r for r in rules if target.lower() in r.lower()]
    rules = [r for r in rules if target.lower() not in r.lower()]

if removed:
    for r in removed:
        if r not in disabled:
            disabled.append(r)
    json.dump(rules, open('${RULES_FILE}', 'w'), indent=2)
    json.dump(disabled, open('${DISABLED_FILE}', 'w'), indent=2)
    for r in removed:
        print(f'Disabled: {r}')
    print(f'{len(rules)} rules remaining')
    print(f'Re-enable with: savants guard enable <number>')
else:
    print(f'No rules matching \"{target}\"')
    print('Run: savants guard list')
"
    ;;

  enable)
    # Re-enable a previously disabled rule by number or text
    RULE_ID="${1:-}"
    if [ -z "$RULE_ID" ]; then
      echo "Usage:"
      echo "  savants guard enable 1           # re-enable disabled rule #1"
      echo "  savants guard enable 'rm -rf'    # re-enable rules matching 'rm -rf'"
      echo ""
      echo "See disabled rules: savants guard disabled"
      exit 1
    fi

    DISABLED_FILE="${SAVANTS_DIR}/disabled-rules.json"

    if [ ! -f "$DISABLED_FILE" ]; then
      echo "No disabled rules found."
      exit 0
    fi

    python3 -c "
import json, sys

disabled = json.load(open('${DISABLED_FILE}'))
if not disabled:
    print('No disabled rules to re-enable.')
    sys.exit(0)

try:
    rules = json.load(open('${RULES_FILE}'))
except:
    rules = []

target = '''${RULE_ID}'''
restored = []

# Try as number first
try:
    idx = int(target) - 1
    if 0 <= idx < len(disabled):
        restored = [disabled.pop(idx)]
    else:
        print(f'Disabled rule #{idx+1} not found. You have {len(disabled)} disabled rules.')
        sys.exit(1)
except ValueError:
    # Try as substring match
    restored = [r for r in disabled if target.lower() in r.lower()]
    disabled = [r for r in disabled if target.lower() not in r.lower()]

if restored:
    for r in restored:
        if r not in rules:
            rules.append(r)
    json.dump(rules, open('${RULES_FILE}', 'w'), indent=2)
    json.dump(disabled, open('${DISABLED_FILE}', 'w'), indent=2)
    for r in restored:
        print(f'Re-enabled: {r}')
    print(f'{len(rules)} rules active')
else:
    print(f'No disabled rules matching \"{target}\"')
    print('Run: savants guard disabled')
"
    ;;

  disabled)
    # List all disabled rules
    DISABLED_FILE="${SAVANTS_DIR}/disabled-rules.json"

    if [ ! -f "$DISABLED_FILE" ]; then
      echo "No disabled rules."
      exit 0
    fi

    python3 -c "
import json
disabled = json.load(open('${DISABLED_FILE}'))
if not disabled:
    print('No disabled rules.')
else:
    print(f'{len(disabled)} disabled rules:')
    print()
    for i, r in enumerate(disabled, 1):
        print(f'  {i:>2}. {r}')
    print()
    print('Re-enable: savants guard enable <number>')
"
    ;;

  reset)
    rm -f "$RULES_FILE"
    rm -f "${SAVANTS_DIR}/guard-paused"
    echo "Guard rules cleared. No protection active."
    echo "Run: savants guard preset standard"
    ;;

  routing)
    ROUTING_FILE="${SAVANTS_DIR}/smart-routing.enabled"
    ACTION="${1:-status}"
    case "$ACTION" in
      on)
        touch "$ROUTING_FILE"
        echo "Smart routing ENABLED."
        echo "  grep → semantic_search, read → file_skeleton"
        echo "  Savants will redirect code search to indexed tools."
        ;;
      off)
        rm -f "$ROUTING_FILE"
        echo "Smart routing DISABLED."
        echo "  grep, read, cat work normally. Only guard rules are active."
        ;;
      *)
        if [ -f "$ROUTING_FILE" ]; then
          echo "Smart routing: ON"
          echo "  Turn off: savants guard routing off"
        else
          echo "Smart routing: OFF (default)"
          echo "  Turn on:  savants guard routing on"
        fi
        ;;
    esac
    ;;

  profiles)
    echo "Available guard profiles:"
    echo ""
    echo "  Core profiles:"
    echo "  minimal            10 rules  Catastrophic actions only"
    echo "  standard           25 rules  Recommended for daily use"
    echo "  paranoid           26 rules  Maximum safety"
    echo "  comprehensive     232 rules  Everything (all categories combined)"
    echo ""
    echo "  Category profiles:"
    echo "  filesystem-safe    21 rules  File system destruction (rm -rf, dd, mkfs, shred)"
    echo "  credentials-safe   56 rules  Sensitive files, API keys, SSH keys, .env, secrets"
    echo "  git-safe           20 rules  Git force push, reset hard, branch delete, filter-branch"
    echo "  database-safe      16 rules  DROP, DELETE/UPDATE without WHERE, TRUNCATE, Redis FLUSH"
    echo "  k8s-safe           26 rules  Kubernetes, Docker, Helm (namespace delete, privileged)"
    echo "  cloud-safe         17 rules  AWS, GCP, Azure, Terraform, Pulumi destroy/delete"
    echo "  network-safe       22 rules  curl+secrets, curl|sh, reverse tunnels, ngrok, scp"
    echo "  publish-safe       16 rules  npm/PyPI/crates.io publish, supply chain attacks"
    echo "  system-safe        20 rules  chmod 777, useradd, iptables, reboot, kill"
    echo "  cicd-safe           8 rules  Workflow edits, secret deletion, Makefile modification"
    echo "  persistence-safe   14 rules  Reverse shells, crontab, systemd units, shell rc files"
    echo ""
    echo "  Legacy profiles:"
    echo "  secrets            27 rules  Credential and token protection (use credentials-safe)"
    echo "  infra-safe         13 rules  Infrastructure protection (use k8s-safe+cloud-safe)"
    echo ""
    echo "Combine with +: savants guard preset standard+credentials-safe+git-safe"
    ;;

  browse)
    # Browse popular guard profiles from cloud
    TAG_FILTER=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --tag) TAG_FILTER="$2"; shift 2 ;;
        *) shift ;;
      esac
    done

    if [ -n "$TAG_FILTER" ]; then
      API_URL="${CLOUD_API}/browse?tag=${TAG_FILTER}"
    else
      API_URL="${CLOUD_API}/browse"
    fi

    RESPONSE=$(curl -fsSL "$API_URL" 2>/dev/null)
    if [ $? -ne 0 ]; then
      echo "  Could not reach savants.cloud"
      exit 1
    fi

    python3 -c "
import json, sys

data = json.load(sys.stdin)
profiles = data if isinstance(data, list) else data.get('profiles', [])

if not profiles:
    print('No profiles found.')
    sys.exit(0)

tag = '${TAG_FILTER}'
if tag:
    print(f'Guard Profiles (tag: {tag})')
else:
    print('Popular Guard Profiles')
print()

for p in profiles:
    handle = p.get('handle', p.get('name', '?'))
    version = p.get('version', '?')
    rules = p.get('rule_count', 0)
    installs = p.get('installs', 0)
    desc = p.get('description', '')

    # Format install count
    if installs >= 1000:
        inst_str = f'{installs/1000:.1f}K'
    else:
        inst_str = str(installs)

    print(f'  {handle:<25} v{version:<6} {rules:>3} rules  {inst_str:>6} installs')
    if desc:
        print(f'    {desc}')
    print()

print('Install: savants guard install @owner/name')
" <<< "$RESPONSE"
    ;;

  update)
    # Update installed cloud profiles
    TARGET="${1:-}"
    CHECK_ONLY=false

    while [[ $# -gt 0 ]]; do
      case "$1" in
        --check) CHECK_ONLY=true; shift ;;
        *) if [ -z "$TARGET" ]; then TARGET="$1"; fi; shift ;;
      esac
    done

    if [ ! -f "$LOCK_FILE" ]; then
      echo "No profiles.lock found. Install a profile first:"
      echo "  savants guard install @owner/name"
      exit 1
    fi

    python3 -c "
import json, sys, subprocess

lock = json.load(open('${LOCK_FILE}'))
target = '${TARGET}'
check_only = '${CHECK_ONLY}' == 'true'
cloud_api = '${CLOUD_API}'

if not lock:
    print('No installed profiles to update.')
    sys.exit(0)

updates = []
for handle, entry in lock.items():
    if target and handle != target and handle != f'@{target}':
        continue

    pinned = entry.get('pinned', entry.get('version', ''))
    current = entry.get('version', '')

    # Query cloud for latest matching version
    if pinned.startswith('^') or pinned.startswith('~'):
        api_url = f'{cloud_api}/{handle.lstrip(\"@\")}/{pinned}'
    else:
        api_url = f'{cloud_api}/{handle.lstrip(\"@\")}'

    try:
        result = subprocess.run(
            ['curl', '-fsSL', api_url],
            capture_output=True, text=True, timeout=10
        )
        if result.returncode != 0:
            print(f'  {handle}: could not check (API unreachable)')
            continue
        data = json.loads(result.stdout)
        latest = data.get('version', '')
        rules = data.get('rules', [])
    except Exception as e:
        print(f'  {handle}: check failed ({e})')
        continue

    if latest and latest != current:
        updates.append({
            'handle': handle,
            'current': current,
            'latest': latest,
            'pinned': pinned,
            'rules': rules,
        })
    else:
        print(f'  {handle}: up to date ({current})')

if not updates:
    print()
    print('All profiles are up to date.')
    sys.exit(0)

print()
for u in updates:
    if check_only:
        print(f'  {u[\"handle\"]}: {u[\"current\"]} -> {u[\"latest\"]} (update available)')
    else:
        handle = u['handle']
        name = handle.lstrip('@').split('/')[-1]
        dest = f'${SAVANTS_DIR}/custom-profiles/{name}.json'

        json.dump(u['rules'], open(dest, 'w'), indent=2)
        rule_count = len(u['rules'])

        # Update lock
        lock[handle]['previous'] = u['current']
        lock[handle]['version'] = u['latest']
        lock[handle]['installed'] = subprocess.run(
            ['date', '-u', '+%Y-%m-%d'], capture_output=True, text=True
        ).stdout.strip()

        print(f'  {handle}: {u[\"current\"]} -> {u[\"latest\"]} ({rule_count} rules)')

if not check_only and updates:
    json.dump(lock, open('${LOCK_FILE}', 'w'), indent=2)
    print()
    print(f'Updated {len(updates)} profile(s).')

if check_only and updates:
    print()
    print(f'{len(updates)} update(s) available. Run: savants guard update')
"
    ;;

  pin)
    # Pin a profile to an exact version
    PROFILE_NAME="${1:-}"
    PIN_VERSION="${2:-}"

    if [ -z "$PROFILE_NAME" ] || [ -z "$PIN_VERSION" ]; then
      echo "Usage: savants guard pin @owner/name 1.2.0"
      echo ""
      echo "Sets an exact version pin in profiles.lock."
      echo "Re-downloads if current version doesn't match."
      exit 1
    fi

    # Normalize handle
    HANDLE="$PROFILE_NAME"
    if [[ "$HANDLE" != @* ]]; then
      HANDLE="@${HANDLE}"
    fi

    if [ ! -f "$LOCK_FILE" ]; then
      echo "No profiles.lock found. Install the profile first:"
      echo "  savants guard install ${HANDLE}"
      exit 1
    fi

    python3 -c "
import json, sys

lock = json.load(open('${LOCK_FILE}'))
handle = '${HANDLE}'
pin_version = '${PIN_VERSION}'

if handle not in lock:
    print(f'{handle} not found in profiles.lock')
    print(f'Install first: savants guard install {handle}')
    sys.exit(1)

current = lock[handle].get('version', '')
lock[handle]['pinned'] = pin_version

json.dump(lock, open('${LOCK_FILE}', 'w'), indent=2)
print(f'Pinned {handle} to version {pin_version}')

if current != pin_version:
    print(f'Current version ({current}) differs from pin ({pin_version})')
    print(f'Re-downloading...')
    sys.exit(2)  # Signal to re-download
else:
    print(f'Current version already matches pin.')
" 2>/dev/null
    PIN_RESULT=$?

    if [ "$PIN_RESULT" -eq 2 ]; then
      # Re-download the pinned version
      CLEAN_HANDLE="${HANDLE#@}"
      OWNER="${CLEAN_HANDLE%%/*}"
      NAME="${CLEAN_HANDLE#*/}"
      exec "$0" install "@${OWNER}/${NAME}@${PIN_VERSION}"
    fi
    ;;

  why|last-block)
    # Show the last blocked event from hook-stats.jsonl
    if [ ! -f "$STATS_FILE" ]; then
      echo "No guard events recorded yet."
      exit 0
    fi

    python3 -c "
import json, sys
from datetime import datetime, timezone

events = []
for line in open('${STATS_FILE}'):
    try:
        events.append(json.loads(line.strip()))
    except:
        pass

# Find last block event
blocks = [e for e in events if e.get('action') == 'block']
if not blocks:
    print('No blocked events found.')
    sys.exit(0)

last = blocks[-1]
ts = last.get('ts', '')
tool = last.get('tool', '?')
detail = last.get('detail', '')
rule = last.get('matched_rule', last.get('detail', ''))
action = last.get('action', 'block')

# Calculate relative time
try:
    event_time = datetime.fromisoformat(ts.replace('Z', '+00:00'))
    now = datetime.now(timezone.utc)
    delta = now - event_time
    seconds = int(delta.total_seconds())
    if seconds < 60:
        ago = f'{seconds} seconds ago'
    elif seconds < 3600:
        ago = f'{seconds // 60} minutes ago'
    elif seconds < 86400:
        ago = f'{seconds // 3600} hours ago'
    else:
        ago = f'{seconds // 86400} days ago'
except:
    ago = ts

print(f'Last blocked: {ago}')
print(f'  Command: {detail}')
print(f'  Rule: {rule}')
print(f'  Action: {action}')
"
    ;;

  help|*)
    echo "savants guard — composable guardrails for AI coding agents"
    echo ""
    echo "Commands:"
    echo "  preset <profiles>  Set active profiles (e.g. standard+secrets+git-safe)"
    echo "  install <source>   Install a profile (@user/name, community, or URL)"
    echo "  browse             Browse popular profiles on savants.cloud"
    echo "  update [name]      Update installed profiles to latest version"
    echo "  pin @u/n <ver>     Pin a profile to an exact version"
    echo "  share <name>       Share a profile to savants.cloud"
    echo "  versions @u/n      List all versions of a cloud profile"
    echo "  rollback @u/n      Rollback to previous installed version"
    echo "  on                 Resume guard protection"
    echo "  off [duration]     Pause guard (e.g. off 10m, off 1h, off = indefinite)"
    echo "  status             Show guard state (active/paused/inactive)"
    echo "  why / last-block   Show the last blocked event"
    echo "  disable <n|text>   Disable a specific rule (reversible)"
    echo "  enable <n|text>    Re-enable a disabled rule"
    echo "  disabled           List all disabled rules"
    echo "  add <rule>         Add a custom guard rule"
    echo "  remove <rule>      Remove a guard rule"
    echo "  list               Show all active rules"
    echo "  stats              Show guard statistics (blocks, allows)"
    echo "  sync push          Push guard config to cloud"
    echo "  sync pull          Pull guard config from cloud"
    echo "  sync status        Show sync status (local vs cloud)"
    echo "  sync auto on|off   Toggle automatic sync"
    echo "  sync events        Sync guard events to cloud"
    echo "  profiles           List available profiles"
    echo "  routing on|off     Toggle smart code routing"
    echo "  reset              Clear all guard rules"
    echo ""
    echo "Quick start:"
    echo "  savants guard preset standard"
    echo ""
    echo "When blocked:"
    echo "  savants guard off 10m     # pause for 10 minutes"
    echo "  savants guard disable 3   # disable rule #3 only"
    echo "  SAVANTS_GUARD=off claude  # disable for one session"
    ;;
esac
