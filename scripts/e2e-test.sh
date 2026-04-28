#!/bin/sh
# Savants E2E Golden Path Tests
# Run: ./scripts/e2e-test.sh
#
# Tests every critical user path against production endpoints.
# No dependencies beyond curl and sh.

set -e

PASS=0
FAIL=0
SKIP=0

API="https://api.savants.cloud"
CLOUD="https://savants.cloud"
SITE="https://savants.dev"
RELEASES="https://releases.savants.dev"
INSTALL="https://savants.sh"

# Colors
if [ -t 1 ]; then
    G='\033[32m'; R='\033[31m'; Y='\033[33m'; B='\033[1m'; D='\033[2m'; X='\033[0m'
else
    G=''; R=''; Y=''; B=''; D=''; X=''
fi

pass() { PASS=$((PASS + 1)); printf "  ${G}PASS${X} %s\n" "$1"; }
fail() { FAIL=$((FAIL + 1)); printf "  ${R}FAIL${X} %s - %s\n" "$1" "$2"; }
skip() { SKIP=$((SKIP + 1)); printf "  ${Y}SKIP${X} %s - %s\n" "$1" "$2"; }

http_status() {
    curl -o /dev/null -w "%{http_code}" --max-time 10 -s "$@" 2>/dev/null
}

http_body() {
    curl -sf --max-time 10 "$@" 2>/dev/null || echo ""
}

# ═══════════════════════════════════════════════════════════════════════
printf "\n${B}Savants E2E Golden Path Tests${X}\n"
printf "${D}%s${X}\n\n" "$(date -u '+%Y-%m-%d %H:%M:%S UTC')"

# ─── PATH 1: Website ──────────────────────────────────────────────────
printf "${B}1. Website (savants.dev)${X}\n"

STATUS=$(http_status "$SITE")
[ "$STATUS" = "200" ] && pass "Homepage returns 200" || fail "Homepage" "got $STATUS"

BODY=$(http_body "$SITE")
echo "$BODY" | grep -q "savants" && pass "Homepage contains 'savants'" || fail "Homepage content" "missing brand"

STATUS=$(http_status "$SITE/case-study")
[ "$STATUS" = "200" ] || [ "$STATUS" = "308" ] && pass "Case study page accessible ($STATUS)" || fail "Case study" "got $STATUS"

echo ""

# ─── PATH 2: Install flow ─────────────────────────────────────────────
printf "${B}2. Install flow (curl savants.sh | sh)${X}\n"

STATUS=$(http_status "$INSTALL")
[ "$STATUS" = "200" ] && pass "savants.sh returns 200" || fail "savants.sh" "got $STATUS"

BODY=$(http_body "$INSTALL")
echo "$BODY" | grep -q "#!/bin/sh" && pass "Install script is valid shell" || fail "Install script" "not a shell script"
echo "$BODY" | grep -q "detect_platform" && pass "Install script has platform detection" || fail "Install script" "missing detect_platform"

echo ""

# ─── PATH 3: Release CDN ──────────────────────────────────────────────
printf "${B}3. Release CDN (releases.savants.dev)${X}\n"

VERSION=$(http_body "$RELEASES/latest/version.txt")
[ -n "$VERSION" ] && pass "version.txt returns '$VERSION'" || fail "version.txt" "empty"

STATUS=$(http_status "$RELEASES/install.sh")
[ "$STATUS" = "200" ] && pass "R2 install.sh returns 200" || fail "R2 install.sh" "got $STATUS"

STATUS=$(http_status "$RELEASES/latest/savants-x86_64-unknown-linux-gnu.tar.gz")
[ "$STATUS" = "200" ] && pass "x86_64 Linux binary available" || fail "x86_64 Linux binary" "got $STATUS"

STATUS=$(http_status "$RELEASES/latest/savants-aarch64-unknown-linux-gnu.tar.gz")
[ "$STATUS" = "200" ] && pass "aarch64 Linux binary available" || skip "aarch64 Linux binary" "not built yet"

STATUS=$(http_status "$RELEASES/latest/savants-x86_64-apple-darwin.tar.gz")
[ "$STATUS" = "200" ] && pass "x86_64 macOS binary available" || skip "x86_64 macOS binary" "not built yet"

STATUS=$(http_status "$RELEASES/latest/savants-aarch64-apple-darwin.tar.gz")
[ "$STATUS" = "200" ] && pass "aarch64 macOS binary available" || skip "aarch64 macOS binary" "not built yet"

# Versioned path
STATUS=$(http_status "$RELEASES/v${VERSION}/savants-x86_64-unknown-linux-gnu.tar.gz")
[ "$STATUS" = "200" ] && pass "Versioned binary (v${VERSION}) available" || fail "Versioned binary" "got $STATUS"

echo ""

# ─── PATH 4: API Health ───────────────────────────────────────────────
printf "${B}4. API Health${X}\n"

BODY=$(http_body "$API/health")
echo "$BODY" | grep -q '"ok"' && pass "API health returns ok" || fail "API health" "$BODY"

BODY=$(http_body "$CLOUD/health")
echo "$BODY" | grep -q '"ok"' && pass "Cloud health returns ok" || fail "Cloud health" "$BODY"

echo ""

# ─── PATH 5: Tool list (public) ───────────────────────────────────────
printf "${B}5. Tool list (public, no auth)${X}\n"

BODY=$(http_body "$API/api/v1/tools")
echo "$BODY" | grep -q '"tools"' && pass "Tool list returns tools array" || fail "Tool list" "missing tools"
echo "$BODY" | grep -q 'diagnose_error' && pass "diagnose_error tool listed" || fail "Tool list" "missing diagnose_error"
echo "$BODY" | grep -q 'pr_risk' && pass "pr_risk tool listed" || fail "Tool list" "missing pr_risk"

echo ""

# ─── PATH 6: Auth - protected routes reject unauthenticated ──────────
printf "${B}6. Auth middleware (rejects unauthenticated)${X}\n"

STATUS=$(http_status "$API/api/v1/org")
[ "$STATUS" = "401" ] && pass "GET /org returns 401 without auth" || fail "/org auth" "got $STATUS"

STATUS=$(http_status "$API/api/v1/usage")
[ "$STATUS" = "401" ] && pass "GET /usage returns 401 without auth" || fail "/usage auth" "got $STATUS"

STATUS=$(http_status "$API/api/v1/billing")
[ "$STATUS" = "401" ] && pass "GET /billing returns 401 without auth" || fail "/billing auth" "got $STATUS"

STATUS=$(http_status "$API/api/v1/graphs")
[ "$STATUS" = "401" ] && pass "GET /graphs returns 401 without auth" || fail "/graphs auth" "got $STATUS"

STATUS=$(curl -o /dev/null -w "%{http_code}" --max-time 10 -s -X POST "$API/api/v1/tools/call" -H "Content-Type: application/json" -d '{"tool":"test","input":{}}' 2>/dev/null)
[ "$STATUS" = "401" ] && pass "POST /tools/call returns 401 without auth" || fail "/tools/call auth" "got $STATUS"

echo ""

# ─── PATH 7: Device flow ──────────────────────────────────────────────
printf "${B}7. Device flow (RFC 8628)${X}\n"

DEVICE_RESP=$(curl -sf -X POST "$API/auth/device/code" --max-time 10 2>/dev/null)
DEVICE_CODE=$(echo "$DEVICE_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('device_code',''))" 2>/dev/null)
USER_CODE=$(echo "$DEVICE_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('user_code',''))" 2>/dev/null)
VERIFY_URI=$(echo "$DEVICE_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('verification_uri',''))" 2>/dev/null)

[ -n "$DEVICE_CODE" ] && pass "Device code generated: ${DEVICE_CODE:0:8}..." || fail "Device code" "empty"
[ -n "$USER_CODE" ] && pass "User code generated: $USER_CODE" || fail "User code" "empty"
[ "$VERIFY_URI" = "https://savants.cloud/activate" ] && pass "Verification URI correct" || fail "Verification URI" "$VERIFY_URI"

# Poll should return pending
POLL_STATUS=$(curl -o /dev/null -w "%{http_code}" -s -X POST "$API/auth/device/token" \
  -H "Content-Type: application/json" \
  -d "{\"device_code\":\"$DEVICE_CODE\"}" --max-time 10 2>/dev/null)
[ "$POLL_STATUS" = "428" ] && pass "Token poll returns 428 (authorization_pending)" || fail "Token poll" "got $POLL_STATUS"

echo ""

# ─── PATH 8: Activate page ────────────────────────────────────────────
printf "${B}8. Activate page${X}\n"

STATUS=$(http_status "$CLOUD/activate")
[ "$STATUS" = "200" ] && pass "Activate page returns 200" || fail "Activate page" "got $STATUS"

BODY=$(http_body "$CLOUD/activate?code=$USER_CODE")
echo "$BODY" | grep -q "$USER_CODE" && pass "Activate page shows user code" || fail "Activate page" "missing user code"
echo "$BODY" | grep -q "Google" && pass "Activate page has Google sign-in" || fail "Activate page" "missing Google"
echo "$BODY" | grep -q "GitHub" && pass "Activate page has GitHub sign-in" || fail "Activate page" "missing GitHub"

STATUS=$(http_status "$CLOUD/activate?status=success")
[ "$STATUS" = "200" ] && pass "Success page returns 200" || fail "Success page" "got $STATUS"

BODY=$(http_body "$CLOUD/activate?status=success")
echo "$BODY" | grep -q "Connected" && pass "Success page shows connected message" || fail "Success page" "missing connected"

echo ""

# ─── PATH 9: savants.cloud redirects ──────────────────────────────────
printf "${B}9. savants.cloud redirects${X}\n"

REDIRECT=$(curl -sf -o /dev/null -w "%{redirect_url}" --max-time 10 "$CLOUD/" 2>/dev/null)
echo "$REDIRECT" | grep -q "savants.dev" && pass "Root redirects to savants.dev" || fail "Root redirect" "$REDIRECT"

STATUS=$(http_status "$CLOUD/dashboard")
[ "$STATUS" = "302" ] && pass "Dashboard redirects" || fail "Dashboard redirect" "got $STATUS"

echo ""

# ─── PATH 10: 404 handling ────────────────────────────────────────────
printf "${B}10. Error handling${X}\n"

STATUS=$(curl -o /dev/null -w "%{http_code}" --max-time 10 -s "$API/nonexistent" 2>/dev/null)
[ "$STATUS" = "404" ] && pass "Unknown API route returns 404" || fail "404 handling" "got $STATUS"

BODY=$(curl -s --max-time 10 "$API/nonexistent" 2>/dev/null)
echo "$BODY" | grep -q '"not_found"' && pass "404 returns JSON error" || fail "404 JSON" "not JSON"

echo ""

# ─── SUMMARY ──────────────────────────────────────────────────────────
TOTAL=$((PASS + FAIL + SKIP))
printf "${B}═══════════════════════════════════════════${X}\n"
printf "${B}Results:${X} ${G}${PASS} passed${X}, ${R}${FAIL} failed${X}, ${Y}${SKIP} skipped${X} / ${TOTAL} total\n"

if [ "$FAIL" -gt 0 ]; then
    printf "${R}${B}SOME TESTS FAILED${X}\n"
    exit 1
else
    printf "${G}${B}ALL TESTS PASSED${X}\n"
fi
