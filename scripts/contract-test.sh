#!/bin/sh
# Savants API Contract Tests
#
# Validates the shape of every API response - catches breaking changes.
# Run: ./scripts/contract-test.sh [base_url]
#
# These tests don't check values, they check STRUCTURE.
# If a field is missing or changes type, the contract is broken.

set -e

API="${1:-https://api.savants.cloud}"
PASS=0; FAIL=0

if [ -t 1 ]; then
    G='\033[32m'; R='\033[31m'; B='\033[1m'; D='\033[2m'; X='\033[0m'
else
    G=''; R=''; B=''; D=''; X=''
fi

pass() { PASS=$((PASS + 1)); printf "  ${G}PASS${X} %s\n" "$1"; }
fail() { FAIL=$((FAIL + 1)); printf "  ${R}FAIL${X} %s - %s\n" "$1" "$2"; }

# Helper: check JSON has required fields
check_fields() {
    local label="$1"; local json="$2"; shift 2
    for field in "$@"; do
        if echo "$json" | python3 -c "import sys,json; d=json.load(sys.stdin); assert '$field' in d" 2>/dev/null; then
            pass "$label has '$field'"
        else
            fail "$label missing '$field'" "field not found in response"
        fi
    done
}

# Helper: check JSON array items have required fields
check_array_item_fields() {
    local label="$1"; local json="$2"; local array_key="$3"; shift 3
    for field in "$@"; do
        if echo "$json" | python3 -c "
import sys,json
d=json.load(sys.stdin)
items=d['$array_key']
assert len(items)>0, 'empty array'
assert all('$field' in item for item in items), 'missing field'
" 2>/dev/null; then
            pass "$label[$array_key][*] has '$field'"
        else
            fail "$label[$array_key][*] missing '$field'" ""
        fi
    done
}

printf "\n${B}Savants API Contract Tests${X}\n"
printf "${D}%s against %s${X}\n\n" "$(date -u '+%Y-%m-%d %H:%M:%S UTC')" "$API"

# ─── GET /health ──────────────────────────────────────────────────────
printf "${B}GET /health${X}\n"
RESP=$(curl -sf "$API/health" 2>/dev/null)
check_fields "health" "$RESP" status timestamp

echo ""

# ─── GET /api/v1/tools ────────────────────────────────────────────────
printf "${B}GET /api/v1/tools${X}\n"
RESP=$(curl -sf "$API/api/v1/tools" 2>/dev/null)
check_fields "tools" "$RESP" tools
check_array_item_fields "tools" "$RESP" tools name description input_schema pricing

# Check input_schema has type and properties
echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
for t in d['tools']:
    s = t['input_schema']
    assert s.get('type') == 'object', f'{t[\"name\"]}: schema type not object'
    assert 'properties' in s, f'{t[\"name\"]}: no properties'
" 2>/dev/null && pass "tools[*].input_schema valid" || fail "tools[*].input_schema" "invalid schema"

# Check pricing has expected fields
check_array_item_fields "tools" "$RESP" tools pricing

echo ""

# ─── POST /auth/device/code ──────────────────────────────────────────
printf "${B}POST /auth/device/code${X}\n"
RESP=$(curl -sf -X POST "$API/auth/device/code" 2>/dev/null)
check_fields "device/code" "$RESP" device_code user_code verification_uri verification_uri_complete expires_in interval

# Type checks
echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
assert isinstance(d['expires_in'], int), 'expires_in not int'
assert isinstance(d['interval'], int), 'interval not int'
assert len(d['user_code']) == 8, 'user_code not 8 chars'
assert '-' in d['device_code'], 'device_code not UUID format'
assert d['verification_uri'].startswith('https://'), 'verification_uri not https'
" 2>/dev/null && pass "device/code types valid" || fail "device/code types" "wrong types"

echo ""

# ─── Error responses ──────────────────────────────────────────────────
printf "${B}Error response contract${X}\n"

# 401 - unauthorized
RESP=$(curl -s "$API/api/v1/org" 2>/dev/null)
check_fields "401 error" "$RESP" error message status
echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
assert d['status'] == 401
assert isinstance(d['error'], str)
assert isinstance(d['message'], str)
" 2>/dev/null && pass "401 error shape valid" || fail "401 error shape" ""

# 404 - not found
RESP=$(curl -s "$API/does-not-exist" 2>/dev/null)
check_fields "404 error" "$RESP" error message status
echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
assert d['status'] == 404
" 2>/dev/null && pass "404 error shape valid" || fail "404 error shape" ""

echo ""

# ─── CORS headers ─────────────────────────────────────────────────────
printf "${B}CORS headers${X}\n"
HEADERS=$(curl -sI "$API/health" 2>/dev/null)
echo "$HEADERS" | grep -qi "access-control-allow" && pass "CORS headers present" || fail "CORS headers" "missing"
echo "$HEADERS" | grep -qi "x-request-id" && pass "X-Request-Id header present" || fail "X-Request-Id" "missing"

echo ""

# ─── Content-Type ─────────────────────────────────────────────────────
printf "${B}Content-Type headers${X}\n"
CT=$(curl -sI "$API/health" 2>/dev/null | grep -i content-type | head -1)
echo "$CT" | grep -qi "application/json" && pass "JSON content-type" || fail "Content-Type" "$CT"

echo ""

# ─── SUMMARY ──────────────────────────────────────────────────────────
TOTAL=$((PASS + FAIL))
printf "${B}═══════════════════════════════════════════${X}\n"
printf "${B}Results:${X} ${G}${PASS} passed${X}, ${R}${FAIL} failed${X} / ${TOTAL} total\n"
[ "$FAIL" -gt 0 ] && printf "${R}${B}CONTRACT BROKEN${X}\n" && exit 1
printf "${G}${B}ALL CONTRACTS VALID${X}\n"
