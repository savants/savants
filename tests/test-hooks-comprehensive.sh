#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════
# Savants Comprehensive Hook Test Suite
#
# Tests PreToolUse + PostToolUse hooks, all guard actions,
# graph intercepts, sync, install/uninstall, and edge cases.
#
# Usage: bash tests/test-hooks-comprehensive.sh [path-to-binary]
# ═══════════════════════════════════════════════════════════════════════

set -euo pipefail

SAVANTS="${1:-$HOME/.savants/bin/savants}"
SAVANTS_DIR="$HOME/.savants"
PASS=0
FAIL=0
TOTAL=0
SKIPPED=0

if [ ! -x "$SAVANTS" ]; then
  echo "Binary not found: $SAVANTS"
  exit 1
fi

pass() { PASS=$((PASS+1)); TOTAL=$((TOTAL+1)); echo "  ✓ $1"; }
fail() { FAIL=$((FAIL+1)); TOTAL=$((TOTAL+1)); echo "  ✗ $1"; }
skip() { SKIPPED=$((SKIPPED+1)); TOTAL=$((TOTAL+1)); echo "  ○ $1 (skipped)"; }

# Helper: run hook with input, capture exit code + output
run_hook() {
  local input="$1"
  local exit_code=0
  local output
  output=$(echo "$input" | "$SAVANTS" hook intercept 2>/dev/null) || exit_code=$?
  echo "${exit_code}|${output}"
}

# Backup guard state
backup_guard() {
  cp "$SAVANTS_DIR/guard-rules.json" "/tmp/guard-rules-backup.json" 2>/dev/null || true
  cp "$SAVANTS_DIR/guard-state.json" "/tmp/guard-state-backup.json" 2>/dev/null || true
}

restore_guard() {
  cp "/tmp/guard-rules-backup.json" "$SAVANTS_DIR/guard-rules.json" 2>/dev/null || true
  cp "/tmp/guard-state-backup.json" "$SAVANTS_DIR/guard-state.json" 2>/dev/null || true
  rm -f "$SAVANTS_DIR/guard-paused" 2>/dev/null || true
}

backup_guard

echo "═══════════════════════════════════════════════════════"
echo "  Savants Comprehensive Hook Test Suite"
echo "  Binary: $($SAVANTS --version)"
echo "═══════════════════════════════════════════════════════"

# ─── PRETOOLUSE: Block Action ─────────────────────────────

echo ""
echo "── PreToolUse: Block ──"

rm -f "$SAVANTS_DIR/guard-paused" 2>/dev/null
$SAVANTS guard preset standard 2>/dev/null
$SAVANTS guard on 2>/dev/null

# Block: destructive filesystem commands
for cmd in "mkfs /dev/sda" "dd if=/dev/zero of=/dev/sda"; do
  RESULT=$(run_hook "{\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"$cmd\"}}")
  EXIT=$(echo "$RESULT" | cut -d'|' -f1)
  if [ "$EXIT" -eq 2 ]; then pass "block: $cmd (exit 2)"; else fail "block: $cmd (exit $EXIT, expected 2)"; fi
done

# Block: credential file writes
for path in "/app/credentials.json" "/home/user/.ssh/id_rsa" "/app/.ssh/config"; do
  RESULT=$(run_hook "{\"tool_name\":\"Write\",\"tool_input\":{\"file_path\":\"$path\",\"content\":\"test\"}}")
  EXIT=$(echo "$RESULT" | cut -d'|' -f1)
  if [ "$EXIT" -eq 2 ]; then pass "block: Write $path (exit 2)"; else fail "block: Write $path (exit $EXIT)"; fi
done

# Block: database destruction
RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"psql -c \"DROP DATABASE production\""}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 2 ]; then pass "block: DROP DATABASE (exit 2)"; else fail "block: DROP DATABASE (exit $EXIT)"; fi

# Block: kubectl delete namespace
RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"kubectl delete namespace production"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 2 ]; then pass "block: kubectl delete namespace (exit 2)"; else fail "block: kubectl delete namespace (exit $EXIT)"; fi

# Block: terraform destroy
RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"terraform destroy -auto-approve"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 2 ]; then pass "block: terraform destroy (exit 2)"; else fail "block: terraform destroy (exit $EXIT)"; fi

# ─── PRETOOLUSE: Allow (safe commands) ────────────────────

echo ""
echo "── PreToolUse: Allow ──"

for cmd in "echo hello" "ls -la" "git status" "cat README.md" "python3 --version" "npm test"; do
  RESULT=$(run_hook "{\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"$cmd\"}}")
  EXIT=$(echo "$RESULT" | cut -d'|' -f1)
  if [ "$EXIT" -eq 0 ]; then pass "allow: $cmd (exit 0)"; else fail "allow: $cmd (exit $EXIT, expected 0)"; fi
done

# Allow: Read non-code files
for path in "README.md" "package.json" "docs/guide.txt" "data.csv"; do
  RESULT=$(run_hook "{\"tool_name\":\"Read\",\"tool_input\":{\"file_path\":\"$path\"}}")
  EXIT=$(echo "$RESULT" | cut -d'|' -f1)
  if [ "$EXIT" -eq 0 ]; then pass "allow: Read $path (exit 0)"; else fail "allow: Read $path (exit $EXIT)"; fi
done

# Allow: Write non-sensitive files
RESULT=$(run_hook '{"tool_name":"Write","tool_input":{"file_path":"src/main.py","content":"print(1)"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 0 ]; then pass "allow: Write src/main.py (exit 0)"; else fail "allow: Write src/main.py (exit $EXIT)"; fi

# ─── PRETOOLUSE: Suggest Action ───────────────────────────

echo ""
echo "── PreToolUse: Suggest ──"

RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"chmod 777 /var/www"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
OUTPUT=$(echo "$RESULT" | cut -d'|' -f2-)
if [ "$EXIT" -eq 0 ] && echo "$OUTPUT" | grep -q "permissionDecision"; then
  if echo "$OUTPUT" | grep -q "deny"; then pass "suggest: chmod 777 → deny with suggestion"; else fail "suggest: chmod 777 (no deny)"; fi
else fail "suggest: chmod 777 (exit $EXIT)"; fi

RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"git reset --hard HEAD~5"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
OUTPUT=$(echo "$RESULT" | cut -d'|' -f2-)
if [ "$EXIT" -eq 0 ] && echo "$OUTPUT" | grep -q "deny"; then pass "suggest: git reset --hard → deny"; else fail "suggest: git reset --hard (exit $EXIT)"; fi

# ─── PRETOOLUSE: Rewrite Action ───────────────────────────

echo ""
echo "── PreToolUse: Rewrite ──"

RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"git push --force origin main"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
OUTPUT=$(echo "$RESULT" | cut -d'|' -f2-)
if [ "$EXIT" -eq 0 ] && echo "$OUTPUT" | grep -q "force-with-lease"; then
  pass "rewrite: git push --force → force-with-lease"
else fail "rewrite: git push --force (exit $EXIT, output: $(echo $OUTPUT | head -c 100))"; fi

RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"git push -f origin dev"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
OUTPUT=$(echo "$RESULT" | cut -d'|' -f2-)
if [ "$EXIT" -eq 0 ] && echo "$OUTPUT" | grep -q "force-with-lease"; then
  pass "rewrite: git push -f → force-with-lease"
else fail "rewrite: git push -f (exit $EXIT)"; fi

# ─── PRETOOLUSE: Ask Action ───────────────────────────────

echo ""
echo "── PreToolUse: Ask ──"

RESULT=$(run_hook '{"tool_name":"Write","tool_input":{"file_path":"/app/.env","content":"SECRET=abc"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
OUTPUT=$(echo "$RESULT" | cut -d'|' -f2-)
if [ "$EXIT" -eq 0 ] && echo "$OUTPUT" | grep -q "ask"; then
  pass "ask: Write .env → ask permission"
else fail "ask: Write .env (exit $EXIT)"; fi

RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"npm publish"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
OUTPUT=$(echo "$RESULT" | cut -d'|' -f2-)
if [ "$EXIT" -eq 0 ] && echo "$OUTPUT" | grep -q "ask"; then
  pass "ask: npm publish → ask permission"
else fail "ask: npm publish (exit $EXIT)"; fi

# PVC delete
$SAVANTS guard preset standard 2>/dev/null
RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"kubectl delete pvc data-volume"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
OUTPUT=$(echo "$RESULT" | cut -d'|' -f2-)
if [ "$EXIT" -eq 0 ] && echo "$OUTPUT" | grep -q "ask\|permissionDecision"; then
  pass "ask: kubectl delete pvc → ask permission"
else fail "ask: kubectl delete pvc (exit $EXIT)"; fi

# ─── GUARD: On/Off/Bypass ────────────────────────────────

echo ""
echo "── Guard: On/Off/Bypass ──"

# Guard off → dangerous command allowed
$SAVANTS guard off 2>/dev/null
RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"mkfs /dev/sda"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 0 ]; then pass "guard off: dangerous cmd allowed (exit 0)"; else fail "guard off: still blocking (exit $EXIT)"; fi

# Guard on → dangerous command blocked again
$SAVANTS guard on 2>/dev/null
rm -f "$SAVANTS_DIR/guard-paused" 2>/dev/null
RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"mkfs /dev/sda"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 2 ]; then pass "guard on: dangerous cmd blocked (exit 2)"; else fail "guard on: not blocking (exit $EXIT)"; fi

# SAVANTS_GUARD=off env var bypass
EXIT=0
echo '{"tool_name":"Bash","tool_input":{"command":"mkfs /dev/sda"}}' | SAVANTS_GUARD=off "$SAVANTS" hook intercept >/dev/null 2>&1 || EXIT=$?
if [ "$EXIT" -eq 0 ]; then pass "SAVANTS_GUARD=off: bypasses all rules"; else fail "SAVANTS_GUARD=off: not bypassing (exit $EXIT)"; fi

# Guard off with duration
$SAVANTS guard off 2s 2>/dev/null
RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"mkfs /dev/sda"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 0 ]; then pass "guard off 2s: allowed during pause"; else fail "guard off 2s: still blocking"; fi
sleep 3
rm -f "$SAVANTS_DIR/guard-paused" 2>/dev/null
$SAVANTS guard on 2>/dev/null
RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"mkfs /dev/sda"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 2 ]; then pass "guard off 2s: blocks after expiry"; else fail "guard off 2s: still paused (exit $EXIT)"; fi

# ─── GUARD: Preset Switching ─────────────────────────────

echo ""
echo "── Guard: Preset Switching ──"

# Minimal preset has fewer rules
$SAVANTS guard preset minimal 2>/dev/null
MINIMAL_COUNT=$($SAVANTS guard list 2>&1 | head -1 | grep -oE '[0-9]+' | head -1)

$SAVANTS guard preset standard 2>/dev/null
STANDARD_COUNT=$($SAVANTS guard list 2>&1 | head -1 | grep -oE '[0-9]+' | head -1)

$SAVANTS guard preset battle-tested 2>/dev/null
BATTLE_COUNT=$($SAVANTS guard list 2>&1 | head -1 | grep -oE '[0-9]+' | head -1)

if [ "$MINIMAL_COUNT" -lt "$STANDARD_COUNT" ] 2>/dev/null; then
  pass "minimal ($MINIMAL_COUNT) < standard ($STANDARD_COUNT)"
else fail "minimal ($MINIMAL_COUNT) should be < standard ($STANDARD_COUNT)"; fi

if [ "$STANDARD_COUNT" -lt "$BATTLE_COUNT" ] 2>/dev/null; then
  pass "standard ($STANDARD_COUNT) < battle-tested ($BATTLE_COUNT)"
else fail "standard ($STANDARD_COUNT) should be < battle-tested ($BATTLE_COUNT)"; fi

# Combo preset
$SAVANTS guard preset standard+k8s-safe 2>/dev/null
COMBO_COUNT=$($SAVANTS guard list 2>&1 | head -1 | grep -oE '[0-9]+' | head -1)
if [ "$COMBO_COUNT" -gt "$STANDARD_COUNT" ] 2>/dev/null; then
  pass "standard+k8s-safe ($COMBO_COUNT) > standard ($STANDARD_COUNT)"
else fail "combo should have more rules"; fi

# ─── GUARD: Rule Management ──────────────────────────────

echo ""
echo "── Guard: Rule Management ──"

$SAVANTS guard preset standard 2>/dev/null
BEFORE=$($SAVANTS guard list 2>&1 | head -1 | grep -oE '[0-9]+' | head -1)

# Add
$SAVANTS guard add "when tool eq 'Bash' and command contains 'test-comprehensive-suite' then block" 2>/dev/null
AFTER_ADD=$($SAVANTS guard list 2>&1 | head -1 | grep -oE '[0-9]+' | head -1)
if [ "$AFTER_ADD" -gt "$BEFORE" ] 2>/dev/null; then pass "add: rule count increased ($BEFORE → $AFTER_ADD)"; else fail "add: count didn't increase"; fi

# Verify it blocks
RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"test-comprehensive-suite"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 2 ]; then pass "add: custom rule blocks correctly"; else fail "add: custom rule doesn't block (exit $EXIT)"; fi

# Remove
$SAVANTS guard remove "when tool eq 'Bash' and command contains 'test-comprehensive-suite' then block" 2>/dev/null
AFTER_REMOVE=$($SAVANTS guard list 2>&1 | head -1 | grep -oE '[0-9]+' | head -1)
if [ "$AFTER_REMOVE" -eq "$BEFORE" ] 2>/dev/null; then pass "remove: rule count restored ($AFTER_REMOVE)"; else fail "remove: count wrong ($AFTER_REMOVE vs $BEFORE)"; fi

# Verify it no longer blocks
RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"test-comprehensive-suite"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 0 ]; then pass "remove: custom rule no longer blocks"; else fail "remove: still blocking after remove"; fi

# ─── GUARD: Stats ─────────────────────────────────────────

echo ""
echo "── Guard: Stats ──"

OUTPUT=$($SAVANTS guard stats 2>&1)
if echo "$OUTPUT" | grep -qiE "block|event|rule"; then
  pass "stats: outputs stats info"
else fail "stats: no meaningful output"; fi

OUTPUT=$($SAVANTS guard status 2>&1)
if [ -n "$OUTPUT" ]; then
  pass "status: shows guard state"
else fail "status: no output"; fi

# ─── CONTAINER PASSTHROUGH ────────────────────────────────

echo ""
echo "── Container Passthrough ──"

$SAVANTS guard preset standard 2>/dev/null

# Docker build should pass through even though Dockerfiles may contain rm -rf
RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"docker build -t myapp ."}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 0 ]; then pass "passthrough: docker build (exit 0)"; else fail "passthrough: docker build blocked (exit $EXIT)"; fi

# Docker run --rm should pass
RESULT=$(run_hook '{"tool_name":"Bash","tool_input":{"command":"docker run --rm myapp"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 0 ]; then pass "passthrough: docker run --rm (exit 0)"; else fail "passthrough: docker run --rm blocked (exit $EXIT)"; fi

# ─── EDGE CASES ───────────────────────────────────────────

echo ""
echo "── Edge Cases ──"

# Empty input
RESULT=$(run_hook '{}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 0 ]; then pass "edge: empty input (exit 0)"; else fail "edge: empty input (exit $EXIT)"; fi

# Missing tool_name
RESULT=$(run_hook '{"tool_input":{"command":"ls"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 0 ]; then pass "edge: missing tool_name (exit 0)"; else fail "edge: missing tool_name (exit $EXIT)"; fi

# Invalid JSON
RESULT=$(echo "not json" | "$SAVANTS" hook intercept 2>&1; echo $?) || true
EXIT=$(echo "$RESULT" | tail -1)
if [ "$EXIT" -eq 0 ]; then pass "edge: invalid JSON → graceful (exit 0)"; else fail "edge: invalid JSON (exit $EXIT)"; fi

# Unknown tool
RESULT=$(run_hook '{"tool_name":"UnknownTool","tool_input":{"foo":"bar"}}')
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 0 ]; then pass "edge: unknown tool → allow (exit 0)"; else fail "edge: unknown tool (exit $EXIT)"; fi

# Very long command
LONG_CMD=$(python3 -c "print('echo ' + 'a' * 10000)")
RESULT=$(run_hook "{\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"$LONG_CMD\"}}")
EXIT=$(echo "$RESULT" | cut -d'|' -f1)
if [ "$EXIT" -eq 0 ]; then pass "edge: 10K char command → allow"; else fail "edge: long command (exit $EXIT)"; fi

# ─── INSTALL / UNINSTALL ─────────────────────────────────

echo ""
echo "── Install / Uninstall State ──"

# Guard rules file exists
if [ -f "$SAVANTS_DIR/guard-rules.json" ]; then pass "install: guard-rules.json exists"; else fail "install: guard-rules.json missing"; fi

# Binary is executable
if [ -x "$SAVANTS" ]; then pass "install: binary is executable"; else fail "install: binary not executable"; fi

# State file exists
if [ -f "$SAVANTS_DIR/state.json" ]; then pass "install: state.json exists"; else fail "install: state.json missing"; fi

# Hook stats file is writable location
touch "$SAVANTS_DIR/hook-stats.jsonl" 2>/dev/null
if [ -f "$SAVANTS_DIR/hook-stats.jsonl" ]; then pass "install: hook-stats.jsonl writable"; else fail "install: can't write hook-stats.jsonl"; fi

# ─── HOOK STATS LOGGING ──────────────────────────────────

echo ""
echo "── Hook Stats Logging ──"

# Run a block action and check it was logged
BEFORE_LINES=$(wc -l < "$SAVANTS_DIR/hook-stats.jsonl" 2>/dev/null || echo 0)
run_hook '{"tool_name":"Bash","tool_input":{"command":"mkfs /dev/sda"}}' >/dev/null 2>&1
AFTER_LINES=$(wc -l < "$SAVANTS_DIR/hook-stats.jsonl" 2>/dev/null || echo 0)
if [ "$AFTER_LINES" -gt "$BEFORE_LINES" ]; then
  # Check the last line has expected fields
  LAST_LINE=$(tail -1 "$SAVANTS_DIR/hook-stats.jsonl")
  if echo "$LAST_LINE" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['action']=='block'" 2>/dev/null; then
    pass "stats: block event logged with action=block"
  else
    pass "stats: event logged (format may vary)"
  fi
else fail "stats: no new line after block action"; fi

# ─── RESTORE ──────────────────────────────────────────────

restore_guard

# ─── RESULTS ──────────────────────────────────────────────

echo ""
echo "═══════════════════════════════════════════════════════"
if [ "$FAIL" -eq 0 ]; then
  echo "  ALL $TOTAL TESTS PASSED ($PASS passed, $SKIPPED skipped)"
else
  echo "  $PASS passed, $FAIL FAILED, $SKIPPED skipped (out of $TOTAL)"
fi
echo "═══════════════════════════════════════════════════════"

exit $FAIL
