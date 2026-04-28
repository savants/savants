#!/bin/sh
# Production health monitor
# Run on cron: */5 * * * * /path/to/monitor.sh
# Or: ./scripts/monitor.sh [webhook_url]
#
# Checks all production endpoints. Sends alert on failure.

WEBHOOK_URL="${1:-}"
FAILURES=""

check() {
    local name="$1" url="$2" expected_status="$3"
    STATUS=$(curl -o /dev/null -w "%{http_code}" -s --max-time 10 "$url" 2>/dev/null)
    if [ "$STATUS" != "$expected_status" ]; then
        FAILURES="${FAILURES}FAIL: ${name} (expected ${expected_status}, got ${STATUS})\n"
    fi
}

check "savants.dev"              "https://savants.dev"                           200
check "api.savants.cloud health" "https://api.savants.cloud/health"              200
check "savants.cloud redirect"   "https://savants.cloud"                         302
check "savants.sh installer"     "https://savants.sh"                            200
check "tool list"                "https://api.savants.cloud/api/v1/tools"        200
check "releases version.txt"     "https://releases.savants.dev/latest/version.txt" 200
check "activate page"            "https://savants.cloud/activate"                200
check "auth rejects unauthed"    "https://api.savants.cloud/api/v1/org"          401

if [ -n "$FAILURES" ]; then
    MSG="SAVANTS PRODUCTION ALERT\n$(date -u)\n\n${FAILURES}"
    printf "$MSG" >&2

    if [ -n "$WEBHOOK_URL" ]; then
        curl -sf -X POST "$WEBHOOK_URL" \
            -H "Content-Type: application/json" \
            -d "{\"text\":\"$(printf "$MSG" | sed 's/"/\\"/g')\"}" >/dev/null 2>&1
    fi
    exit 1
fi

echo "$(date -u) - All 8 checks passed"
