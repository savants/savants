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
PROFILES_DIR="$(dirname "$0")/../packages/guard-profiles/presets"

# If installed globally, profiles are bundled
if [ ! -d "$PROFILES_DIR" ]; then
  PROFILES_DIR="${SAVANTS_DIR}/profiles"
fi

cmd="${1:-help}"
shift || true

case "$cmd" in
  preset)
    # Parse profile names: standard+secrets+git-safe
    PRESET_STR="${1:-standard}"
    IFS='+' read -ra PROFILES <<< "$PRESET_STR"

    ALL_RULES="[]"
    LOADED=""

    for profile in "${PROFILES[@]}"; do
      PROFILE_FILE="${PROFILES_DIR}/${profile}.json"
      if [ ! -f "$PROFILE_FILE" ]; then
        echo "Unknown profile: ${profile}"
        echo "Available: minimal, standard, paranoid, comprehensive, filesystem-safe, credentials-safe,"
        echo "  git-safe, database-safe, k8s-safe, cloud-safe, network-safe, publish-safe,"
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
    # Sync local hook-stats.jsonl to Savants Cloud
    API_KEY="${SAVANTS_API_KEY:-}"
    if [ -z "$API_KEY" ]; then
      STATE_FILE="${SAVANTS_DIR}/state.json"
      if [ -f "$STATE_FILE" ]; then
        API_KEY=$(python3 -c "import json; print(json.load(open('$STATE_FILE')).get('cloud_token',''))" 2>/dev/null)
      fi
    fi

    if [ -z "$API_KEY" ]; then
      echo "No API key found. Set SAVANTS_API_KEY or run savants cloud login."
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
            'Authorization': 'Bearer ${API_KEY}',
            'Content-Type': 'application/json',
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

    # Cloud sync status
    STATE_FILE="${SAVANTS_DIR}/state.json"
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
    # Disable a specific rule by number or substring
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

    python3 -c "
import json, sys
rules = json.load(open('${RULES_FILE}'))
target = '''${RULE_ID}'''

# Try as number first
try:
    idx = int(target) - 1
    if 0 <= idx < len(rules):
        removed = rules.pop(idx)
        json.dump(rules, open('${RULES_FILE}', 'w'), indent=2)
        print(f'Disabled rule #{idx+1}: {removed}')
        print(f'{len(rules)} rules remaining')
        sys.exit(0)
    else:
        print(f'Rule #{idx+1} not found. You have {len(rules)} rules.')
        sys.exit(1)
except ValueError:
    pass

# Try as substring match
removed = [r for r in rules if target.lower() in r.lower()]
kept = [r for r in rules if target.lower() not in r.lower()]
if removed:
    json.dump(kept, open('${RULES_FILE}', 'w'), indent=2)
    for r in removed:
        print(f'Disabled: {r}')
    print(f'{len(kept)} rules remaining')
else:
    print(f'No rules matching \"{target}\"')
    print('Run: savants guard list')
"
    ;;

  enable)
    # Re-enable a previously disabled rule (add it back)
    RULE="$*"
    if [ -z "$RULE" ]; then
      echo "Usage: savants guard enable \"when tool eq 'Bash' and command contains 'rm -rf' then block\""
      exit 1
    fi
    # Delegate to add
    exec "$0" add "$RULE"
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

  help|*)
    echo "savants guard — composable guardrails for AI coding agents"
    echo ""
    echo "Commands:"
    echo "  preset <profiles>  Set active profiles (e.g. standard+secrets+git-safe)"
    echo "  on                 Resume guard protection"
    echo "  off [duration]     Pause guard (e.g. off 10m, off 1h, off = indefinite)"
    echo "  status             Show guard state (active/paused/inactive)"
    echo "  disable <n|text>   Disable a specific rule by number or keyword"
    echo "  enable <rule>      Re-enable a specific rule"
    echo "  add <rule>         Add a custom guard rule"
    echo "  remove <rule>      Remove a guard rule"
    echo "  list               Show all active rules"
    echo "  stats              Show guard statistics (blocks, allows)"
    echo "  sync               Sync local events to Savants Cloud"
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
