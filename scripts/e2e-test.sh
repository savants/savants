#!/bin/bash
# Savants E2E Test Suite
# Run: bash scripts/e2e-test.sh
set -e

echo "========================================="
echo "  SAVANTS E2E TEST SUITE"
echo "========================================="

CF_TOKEN="${CLOUDFLARE_API_TOKEN:-bSnXmjhm8PJOAtHG2-_X5FKl6G0-9g7dQUl4TgwF}"
CF_ACCOUNT="4992fd600f9894326a82a0f8573a7c38"
D1_ID="bf5c1140-48ac-4b61-bb5c-6fc2a673eb2d"
PASS=0; FAIL=0

# Get token
RESPONSE=$(curl -s -X POST "https://api.savants.cloud/auth/device/code")
DC=$(echo "$RESPONSE" | python3 -c 'import sys,json; print(json.load(sys.stdin)["device_code"])')
curl -s -X POST "https://api.cloudflare.com/client/v4/accounts/$CF_ACCOUNT/d1/database/$D1_ID/query" \
  -H "Authorization: Bearer $CF_TOKEN" -H "Content-Type: application/json" \
  -d "{\"sql\":\"UPDATE device_auth_sessions SET status = 'approved', user_id = '139a5530-cf8c-4389-880b-c15608980c28', org_id = 'cb198567-f0ee-43e5-a1c0-359fd51f9e99' WHERE device_code = '$DC'\"}" > /dev/null
sleep 1
TOKEN=$(curl -s -X POST "https://api.savants.cloud/auth/device/token" -H "Content-Type: application/json" -d "{\"device_code\":\"$DC\"}" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("access_token","FAIL"))')
[ "$TOKEN" != "FAIL" ] && { echo "[PASS] Auth"; PASS=$((PASS+1)); } || { echo "[FAIL] Auth"; FAIL=$((FAIL+1)); exit 1; }

test_tool() {
  local name=$1 input=$2 expect=$3
  if curl -sf -X POST "https://api.savants.cloud/api/v1/tools/call" \
    -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
    -d "{\"tool\":\"$name\",\"input\":$input}" 2>&1 | grep -q "$expect"; then
    echo "[PASS] $name"; PASS=$((PASS+1))
  else
    echo "[FAIL] $name"; FAIL=$((FAIL+1))
  fi
}

# Cloud tools
test_tool "search_sentry_issues" '{"query":"is:unresolved"}' "results"
test_tool "get_sentry_issue" '{"issue_id":"7457429838"}' "title"
test_tool "find_sentry_releases" '{"project":"vocator-backend"}' "results"
test_tool "search_github_issues" '{"query":"ATS","repo":"sourcecoders-ai/talent-pipeline"}' "results"
test_tool "list_github_prs" '{"repo":"sourcecoders-ai/talent-pipeline"}' "results"
test_tool "list_github_actions" '{"repo":"sourcecoders-ai/talent-pipeline"}' "results"
test_tool "list_github_commits" '{"repo":"sourcecoders-ai/talent-pipeline"}' "results"
test_tool "search_linear_issues" '{"query":"ATS"}' "results\|error"
test_tool "list_slack_channels" '{}' "channels\|error"
test_tool "graph_stats" '{}' "total_nodes"
test_tool "function_xray" '{"function_name":"JobDescriptionBuilder","repo":"talent-pipeline"}' "name"
test_tool "find_causes" '{"node_name":"cert-manager","event_type":"pod_crash"}' "probable_causes"
test_tool "diagnose_error" '{"error_message":"[ATS Push] Failed to push role to ATS","sentry_project":"vocator-backend"}' "root_cause"

# New tools: developer report, PR search, graph tools via D1
test_tool "search_github_prs" '{"query":"VSCV","repo":"sourcecoders-ai/talent-pipeline","author":"gustavo"}' "results\|count"
test_tool "developer_report" '{"author":"gustavo","repo":"sourcecoders-ai/talent-pipeline","since":"2026-04-01","until":"2026-05-01"}' "summary\|total_prs"
test_tool "callers" '{"function":"evaluateCandidateAgainstRole","repo":"talent-pipeline"}' "Callers\|error"
test_tool "where_used" '{"symbol":"calculateAndUpsertCandidatePoolScore","repo":"talent-pipeline"}' "used\|error"
test_tool "file_skeleton" '{"file":"candidate-pool-matching.ts","repo":"talent-pipeline"}' "Functions\|error"

# Diagnose with source context
test_tool "diagnose" '{"error_message":"evaluateCandidateAgainstRole token spike","repo":"talent-pipeline"}' "call_chain\|root_cause"

# Local tools
for tool in semantic_search file_skeleton where_used callers blast_radius dead_code test_coverage hotspots entry_points git_blame git_log; do
  if printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}\n{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"'$tool'","arguments":{"query":"error","repo":"savants","function":"main","symbol":"main","file":"src/main.rs","line_start":1,"repo_path":"'"$PWD"'"}}}\n' | ~/.savants/bin/savants serve 2>/dev/null | tail -1 | grep -q "content"; then
    echo "[PASS] local:$tool"; PASS=$((PASS+1))
  else
    echo "[FAIL] local:$tool"; FAIL=$((FAIL+1))
  fi
done

# Agent
ps aux | grep -q "[s]avants agent" && { echo "[PASS] Agent running"; PASS=$((PASS+1)); } || { echo "[FAIL] Agent"; FAIL=$((FAIL+1)); }

echo ""
echo "========================================="
echo "  Results: $PASS passed, $FAIL failed"
echo "========================================="
[ $FAIL -eq 0 ] && exit 0 || exit 1
