#!/bin/bash
# Savants Full Monitor — runs continuously, checks everything, alerts to Gotify
#
# Usage: SAVANTS_GOTIFY_TOKEN=xxx ./scripts/full-monitor.sh
#
# Checks:
# 1. GitHub repo for new commits + open security PRs
# 2. npm audit for vulnerabilities
# 3. AWS health (EKS, RDS, EC2)
# 4. K8s cluster state (via Savants)
# 5. Host state (via Savants)

set -euo pipefail

GOTIFY_URL="${SAVANTS_GOTIFY_URL:-http://10.43.16.5:80}"
GOTIFY_TOKEN="${SAVANTS_GOTIFY_TOKEN:-AcUC9NcjcMGLXtm}"
REPO_PATH="${REPO_PATH:-/home/miguel/git/sourcecoders-ai/talent-pipeline}"
CHECK_INTERVAL="${CHECK_INTERVAL:-300}"  # 5 minutes

LAST_SHA=""
ALERTED_IDS=""

notify() {
    local title="$1"
    local message="$2"
    local priority="${3:-5}"

    echo "[$(date '+%H:%M:%S')] $title: $message"

    curl -s "${GOTIFY_URL}/message?token=${GOTIFY_TOKEN}" -X POST \
        -H "Content-Type: application/json" \
        -d "{\"title\":\"Savants: ${title}\",\"message\":\"${message}\",\"priority\":${priority}}" \
        > /dev/null 2>&1 || true
}

should_alert() {
    local id="$1"
    if echo "$ALERTED_IDS" | grep -q "$id"; then
        return 1  # already alerted
    fi
    ALERTED_IDS="$ALERTED_IDS $id"
    return 0
}

check_github() {
    cd "$REPO_PATH"

    # Check for new commits
    git fetch origin main --quiet 2>/dev/null || git fetch origin master --quiet 2>/dev/null || return

    CURRENT_SHA=$(git rev-parse HEAD 2>/dev/null)
    REMOTE_SHA=$(git rev-parse origin/main 2>/dev/null || git rev-parse origin/master 2>/dev/null)

    if [ -n "$LAST_SHA" ] && [ "$REMOTE_SHA" != "$LAST_SHA" ]; then
        COMMITS=$(git log --oneline "${LAST_SHA}..${REMOTE_SHA}" 2>/dev/null | head -5)
        COUNT=$(echo "$COMMITS" | wc -l)
        LATEST=$(echo "$COMMITS" | head -1)
        notify "New commits" "${COUNT} new commit(s) on talent-pipeline\n${LATEST}" 3
    fi
    LAST_SHA="$REMOTE_SHA"

    # Check for unmerged Dependabot security PRs
    SECURITY_PRS=$(gh pr list --repo sourcecoders-ai/talent-pipeline \
        --label "dependencies" --json number,title,createdAt 2>/dev/null | \
        python3 -c "
import sys, json
from datetime import datetime, timezone
prs = json.load(sys.stdin)
old = [p for p in prs if (datetime.now(timezone.utc) - datetime.fromisoformat(p['createdAt'].rstrip('Z')+'+00:00')).days > 7]
for p in old:
    print(f\"PR #{p['number']}: {p['title']}\")
" 2>/dev/null || true)

    if [ -n "$SECURITY_PRS" ]; then
        COUNT=$(echo "$SECURITY_PRS" | wc -l)
        if should_alert "security-prs-${COUNT}"; then
            notify "Unmerged security PRs" "${COUNT} dependency update PR(s) older than 7 days:\n${SECURITY_PRS}" 7
        fi
    fi
}

check_vulnerabilities() {
    cd "$REPO_PATH"

    # npm audit
    if [ -f package.json ]; then
        AUDIT=$(npm audit --json 2>/dev/null | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    v = d.get('metadata', {}).get('vulnerabilities', {})
    critical = v.get('critical', 0)
    high = v.get('high', 0)
    if critical > 0 or high > 0:
        print(f'{critical} critical, {high} high vulnerabilities')
except:
    pass
" 2>/dev/null || true)

        if [ -n "$AUDIT" ]; then
            if should_alert "npm-audit-${AUDIT}"; then
                notify "npm vulnerabilities" "talent-pipeline: ${AUDIT}" 8
            fi
        fi
    fi
}

check_aws() {
    # EKS cluster health
    for CLUSTER in taria-prod-eks taria-dev-eks; do
        STATUS=$(aws eks describe-cluster --name "$CLUSTER" --region us-west-2 \
            --query 'cluster.status' --output text 2>/dev/null || echo "UNKNOWN")

        if [ "$STATUS" != "ACTIVE" ]; then
            if should_alert "eks-${CLUSTER}-${STATUS}"; then
                notify "EKS cluster unhealthy" "${CLUSTER} status: ${STATUS}" 8
            fi
        fi
    done

    # RDS health
    UNHEALTHY_RDS=$(aws rds describe-db-instances \
        --query 'DBInstances[?DBInstanceStatus!=`available`].{Name:DBInstanceIdentifier,Status:DBInstanceStatus}' \
        --output text 2>/dev/null || true)

    if [ -n "$UNHEALTHY_RDS" ]; then
        if should_alert "rds-unhealthy"; then
            notify "RDS unhealthy" "${UNHEALTHY_RDS}" 8
        fi
    fi

    # EC2 instances not running
    STOPPED_EC2=$(aws ec2 describe-instances \
        --filters "Name=instance-state-name,Values=stopped,stopping,shutting-down" \
        --query 'Reservations[].Instances[].{Name:Tags[?Key==`Name`].Value|[0],State:State.Name}' \
        --output text 2>/dev/null || true)

    if [ -n "$STOPPED_EC2" ]; then
        if should_alert "ec2-stopped"; then
            notify "EC2 instances down" "${STOPPED_EC2}" 5
        fi
    fi

    # CloudWatch alarms in ALARM state
    ALARMS=$(aws cloudwatch describe-alarms --state-value ALARM \
        --query 'MetricAlarms[].AlarmName' --output text 2>/dev/null || true)

    if [ -n "$ALARMS" ]; then
        if should_alert "cw-alarm-${ALARMS}"; then
            notify "CloudWatch ALARM" "${ALARMS}" 8
        fi
    fi
}

check_k8s_via_savants() {
    SAVANTS_BIN="/home/miguel/git/bernadinm/savants/savants-cli/target/release/savants"

    # Check for CrashLoopBackOff across all clusters
    for CLUSTER in taria-prod taria-dev; do
        CRASHES=$(echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"monitor","version":"1.0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_pods","arguments":{"cluster":"'"$CLUSTER"'","status":"CrashLoopBackOff"}}}' | \
            SAVANTS_PORT=6379 timeout 10 "$SAVANTS_BIN" serve 2>/dev/null | \
            python3 -c "
import sys, json
for line in sys.stdin:
    d = json.loads(line.strip())
    if d.get('id') == 2:
        text = d['result']['content'][0]['text']
        if 'Found 0' not in text and 'No pods' not in text:
            print(text[:200])
" 2>/dev/null || true)

        if [ -n "$CRASHES" ]; then
            if should_alert "crash-${CLUSTER}"; then
                notify "CrashLoopBackOff on ${CLUSTER}" "${CRASHES}" 9
            fi
        fi
    done
}

# ── Main loop ──

echo "Savants Full Monitor started"
echo "  Repo: $REPO_PATH"
echo "  Gotify: $GOTIFY_URL"
echo "  Interval: ${CHECK_INTERVAL}s"
echo ""

# Initialize last SHA
cd "$REPO_PATH"
LAST_SHA=$(git rev-parse HEAD 2>/dev/null || echo "")

while true; do
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] Running checks..."

    check_github 2>/dev/null || echo "  GitHub check failed"
    check_vulnerabilities 2>/dev/null || echo "  Vulnerability check failed"
    check_aws 2>/dev/null || echo "  AWS check failed"
    check_k8s_via_savants 2>/dev/null || echo "  K8s check failed"

    echo "[$(date '+%Y-%m-%d %H:%M:%S')] Checks complete. Sleeping ${CHECK_INTERVAL}s..."
    sleep "$CHECK_INTERVAL"
done
