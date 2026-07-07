#!/bin/bash
# Savants Cloud API — End-to-End Test Suite
# Run: bash tests/test-cloud-api.sh
#
# Requires:
#   - ~/.savants/state.json with cloud_token
#   - savants.cloud deployed and accessible
#
# Tests all cloud endpoints: telemetry, guard config sync, profiles, dashboard pages

set -euo pipefail

BASE="${SAVANTS_CLOUD_URL:-https://savants.cloud}"
UA="User-Agent: savants-cli/test"
STATE_FILE="${HOME}/.savants/state.json"

if [ ! -f "$STATE_FILE" ]; then
  echo "ERROR: $STATE_FILE not found. Run 'savants connect' first."
  exit 1
fi

TOKEN=$(python3 -c "import json; print(json.load(open('$STATE_FILE')).get('cloud_token',''))")
if [ -z "$TOKEN" ]; then
  echo "ERROR: No cloud_token in $STATE_FILE"
  exit 1
fi

PASS=0
FAIL=0
TOTAL=0

pass() { PASS=$((PASS+1)); TOTAL=$((TOTAL+1)); echo "  ✓ $1"; }
fail() { FAIL=$((FAIL+1)); TOTAL=$((TOTAL+1)); echo "  ✗ $1"; }

assert_code() {
  local desc="$1" expected="$2" actual="$3"
  if [ "$actual" = "$expected" ]; then pass "$desc (HTTP $actual)"; else fail "$desc (expected $expected, got $actual)"; fi
}

assert_json_field() {
  local desc="$1" json="$2" field="$3"
  local has=$(echo "$json" | python3 -c "import sys,json; d=json.load(sys.stdin); print('yes' if '$field' in d else 'no')" 2>/dev/null)
  if [ "$has" = "yes" ]; then pass "$desc"; else fail "$desc (missing field: $field)"; fi
}

assert_json_expr() {
  local desc="$1" json="$2" expr="$3"
  local result=$(echo "$json" | python3 -c "import sys,json; d=json.load(sys.stdin); print('yes' if $expr else 'no')" 2>/dev/null)
  if [ "$result" = "yes" ]; then pass "$desc"; else fail "$desc"; fi
}

echo "═══════════════════════════════════════════════════════"
echo "  Savants Cloud API — Test Suite"
echo "  Target: $BASE"
echo "═══════════════════════════════════════════════════════"

# ─── TELEMETRY ────────────────────────────────────────────

echo ""
echo "── Telemetry ──"

# POST anonymous event
CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 10 -X POST "$BASE/api/v1/telemetry" \
  -H "Content-Type: application/json" -H "$UA" \
  -d '{"telemetry_id":"sv_test_suite_001","event":"heartbeat","version":"0.21.0","os":"Linux","arch":"x86_64","command":"test"}')
assert_code "POST telemetry (anonymous heartbeat)" "204" "$CODE"

# POST guard event with command_preview
CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 10 -X POST "$BASE/api/v1/telemetry" \
  -H "Content-Type: application/json" -H "$UA" \
  -d '{"telemetry_id":"sv_test_suite_001","user_id":"test-user","event":"guard_block","guard_action":"block","guard_rule":"test-suite-rule","guard_category":"data_destruction","guard_severity":"critical","guard_tool":"Bash","command_preview":"test-dangerous-cmd","version":"0.21.0","os":"Linux"}')
assert_code "POST telemetry (guard_block + command_preview)" "204" "$CODE"

# POST with missing fields (should still 204 — fire-and-forget)
CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 10 -X POST "$BASE/api/v1/telemetry" \
  -H "Content-Type: application/json" -H "$UA" \
  -d '{"telemetry_id":"sv_test_suite_001","event":"heartbeat"}')
assert_code "POST telemetry (minimal fields)" "204" "$CODE"

# POST with empty body (should 204, not error)
CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 10 -X POST "$BASE/api/v1/telemetry" \
  -H "Content-Type: application/json" -H "$UA" \
  -d '{}')
assert_code "POST telemetry (empty body — graceful)" "204" "$CODE"

# GET stats without auth — must be 401
CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 10 "$BASE/api/v1/telemetry/stats" -H "$UA")
assert_code "GET telemetry/stats (no auth)" "401" "$CODE"

# GET stats with auth
STATS=$(curl -s --max-time 10 "$BASE/api/v1/telemetry/stats" -H "Authorization: Bearer $TOKEN" -H "$UA")
assert_json_field "GET telemetry/stats has 'dau'" "$STATS" "dau"
assert_json_field "GET telemetry/stats has 'daily_hosts'" "$STATS" "daily_hosts"
assert_json_field "GET telemetry/stats has 'mau'" "$STATS" "mau"
assert_json_field "GET telemetry/stats has 'total_installs'" "$STATS" "total_installs"
assert_json_field "GET telemetry/stats has 'blocks_prevented'" "$STATS" "blocks_prevented"
assert_json_field "GET telemetry/stats has 'blocks_by_severity'" "$STATS" "blocks_by_severity"
assert_json_field "GET telemetry/stats has 'blocks_by_category'" "$STATS" "blocks_by_category"
assert_json_field "GET telemetry/stats has 'top_rules_fired'" "$STATS" "top_rules_fired"
assert_json_field "GET telemetry/stats has 'recent_blocks'" "$STATS" "recent_blocks"
assert_json_field "GET telemetry/stats has 'estimated_incidents_averted'" "$STATS" "estimated_incidents_averted"
assert_json_expr "GET telemetry/stats dau >= 0" "$STATS" "d.get('dau',0) >= 0"
assert_json_expr "GET telemetry/stats recent_blocks is list" "$STATS" "isinstance(d.get('recent_blocks'), list)"

# Verify command_preview in recent_blocks
assert_json_expr "recent_blocks has command_preview" "$STATS" "any(b.get('command_preview') for b in d.get('recent_blocks',[]))"

# ─── GUARD CONFIG SYNC ───────────────────────────────────

echo ""
echo "── Guard Config Sync ──"

# Push config
PUSH=$(curl -s --max-time 10 -X POST "$BASE/api/v1/guard/config" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" -H "$UA" \
  -d '{"rules":["test-rule-1","test-rule-2"],"preset":"test-preset","machine_id":"test-runner"}')
assert_json_field "POST guard/config (push)" "$PUSH" "version"

# Pull config
PULL=$(curl -s --max-time 10 "$BASE/api/v1/guard/config" \
  -H "Authorization: Bearer $TOKEN" -H "$UA")
assert_json_field "GET guard/config (pull) has 'rules'" "$PULL" "rules"
assert_json_field "GET guard/config (pull) has 'version'" "$PULL" "version"
assert_json_field "GET guard/config (pull) has 'preset'" "$PULL" "preset"

# Version check (lightweight)
VER=$(curl -s --max-time 5 "$BASE/api/v1/guard/config/version" \
  -H "Authorization: Bearer $TOKEN" -H "$UA")
assert_json_field "GET guard/config/version" "$VER" "version"
assert_json_expr "config version > 0" "$VER" "d.get('version', 0) > 0"

# Config without auth
CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 5 "$BASE/api/v1/guard/config" -H "$UA")
assert_code "GET guard/config (no auth)" "401" "$CODE"

# ─── GUARD LOG & STATS ───────────────────────────────────

echo ""
echo "── Guard Log & Stats ──"

# Guard log
LOG=$(curl -s --max-time 10 "$BASE/api/v1/guard/log?limit=5" \
  -H "Authorization: Bearer $TOKEN" -H "$UA")
assert_json_field "GET guard/log" "$LOG" "log"
assert_json_expr "guard/log is array" "$LOG" "isinstance(d.get('log'), list)"

# Guard stats
GSTATS=$(curl -s --max-time 10 "$BASE/api/v1/guard/stats" \
  -H "Authorization: Bearer $TOKEN" -H "$UA")
assert_json_field "GET guard/stats has 'total_events'" "$GSTATS" "total_events"
assert_json_field "GET guard/stats has 'top_rules'" "$GSTATS" "top_rules"

# ─── PROFILES ────────────────────────────────────────────

echo ""
echo "── Profiles ──"

# Browse (public)
CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 10 "$BASE/api/v1/profiles/browse" -H "$UA")
assert_code "GET profiles/browse (public)" "200" "$CODE"

# ─── DASHBOARD PAGES ─────────────────────────────────────

echo ""
echo "── Dashboard Pages ──"

PAGES=("dashboard" "dashboard/telemetry" "dashboard/guard-log" "dashboard/guard-analytics" "dashboard/guard-rules" "dashboard/keys" "dashboard/team" "dashboard/billing" "dashboard/settings")

for PAGE in "${PAGES[@]}"; do
  CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 10 -b "savants_token=$TOKEN" "$BASE/$PAGE")
  assert_code "GET /$PAGE" "200" "$CODE"
done

# ─── DASHBOARD SCRIPT ORDER ──────────────────────────────

echo ""
echo "── Dashboard Script Order ──"

for PAGE in "guard-log" "guard-analytics" "telemetry"; do
  ORDER=$(curl -s --max-time 10 -b "savants_token=$TOKEN" "$BASE/dashboard/$PAGE" | python3 -c "
import sys, re
html = sys.stdin.read()
scripts = re.findall(r'<script>(.*?)</script>', html, re.DOTALL)
gt = next((i for i,s in enumerate(scripts) if 'window.getToken' in s), -1)
# Page script: any script that calls apiFetch or uses guard/telemetry-specific functions
page = next((i for i,s in enumerate(scripts) if ('apiFetch' in s and ('guard' in s or 'telemetry' in s or 'loadLog' in s)) or ('kpiDau' in s and 'getElementById' in s) or ('loadLog' in s and 'function' in s)), -1)
print('correct' if gt >= 0 and page >= 0 and gt < page else 'wrong')
" 2>/dev/null)
  if [ "$ORDER" = "correct" ]; then pass "/$PAGE: getToken defined before page script"; else fail "/$PAGE: wrong script order"; fi
done

# ─── SSR PRELOAD ──────────────────────────────────────────

echo ""
echo "── Server-Side Rendering ──"

PRELOAD=$(curl -s --max-time 10 -b "savants_token=$TOKEN" "$BASE/dashboard/telemetry" | grep -c "__PRELOADED__")
if [ "$PRELOAD" -gt 0 ]; then pass "Telemetry page has __PRELOADED__ data"; else fail "Telemetry page missing __PRELOADED__"; fi

# ─── RESULTS ─────────────────────────────────────────────

echo ""
echo "═══════════════════════════════════════════════════════"
if [ "$FAIL" -eq 0 ]; then
  echo "  ALL $TOTAL TESTS PASSED"
else
  echo "  $PASS passed, $FAIL FAILED out of $TOTAL"
fi
echo "═══════════════════════════════════════════════════════"

exit $FAIL
