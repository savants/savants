#!/bin/bash
# Savants Chaos Test Runner
# Injects known faults into K8s, waits for Savants to diagnose, scores accuracy.
# Fully unattended. Run in CI or manually.
#
# Usage:
#   ./runner.sh --all                    # Run all scenarios
#   ./runner.sh --scenario chaos-dns-001 # Run one scenario
#   ./runner.sh --category infrastructure # Run one category
#   ./runner.sh --dry-run                # Parse scenarios without executing

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SCENARIOS_FILE="$SCRIPT_DIR/scenarios.yaml"
MANIFESTS_DIR="$SCRIPT_DIR/manifests"
RESULTS_DIR="$SCRIPT_DIR/../results"
SAVANTS_BIN="${SAVANTS_BIN:-savants}"
KUBECONFIG="${KUBECONFIG:-/etc/rancher/k3s/k3s.yaml}"
CHAOS_NAMESPACE="chaos-test"

# Colors (disabled in CI)
if [ -t 1 ]; then
  GREEN='\033[32m'; RED='\033[31m'; YELLOW='\033[33m'; CYAN='\033[36m'; BOLD='\033[1m'; RESET='\033[0m'
else
  GREEN=''; RED=''; YELLOW=''; CYAN=''; BOLD=''; RESET=''
fi

log()  { echo -e "${CYAN}[chaos]${RESET} $*"; }
pass() { echo -e "${GREEN}[PASS]${RESET} $*"; }
fail() { echo -e "${RED}[FAIL]${RESET} $*"; }
warn() { echo -e "${YELLOW}[WARN]${RESET} $*"; }

# Parse arguments
RUN_MODE="all"
TARGET=""
DRY_RUN=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --all) RUN_MODE="all"; shift ;;
    --scenario) RUN_MODE="single"; TARGET="$2"; shift 2 ;;
    --category) RUN_MODE="category"; TARGET="$2"; shift 2 ;;
    --dry-run) DRY_RUN=true; shift ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
done

# Ensure dependencies
command -v kubectl >/dev/null || { echo "kubectl not found"; exit 1; }
command -v python3 >/dev/null || { echo "python3 not found"; exit 1; }

# Create results directory
mkdir -p "$RESULTS_DIR"
RESULTS_FILE="$RESULTS_DIR/chaos-$(date +%Y%m%d-%H%M%S).json"

# Parse scenarios from YAML
parse_scenarios() {
  python3 -c "
import yaml, json, sys
with open('$SCENARIOS_FILE') as f:
    data = yaml.safe_load(f)
scenarios = data.get('scenarios', [])
# Filter based on mode
mode = '$RUN_MODE'
target = '$TARGET'
if mode == 'single':
    scenarios = [s for s in scenarios if s['id'] == target]
elif mode == 'category':
    scenarios = [s for s in scenarios if s['category'] == target]
json.dump(scenarios, sys.stdout)
"
}

# Ensure chaos namespace exists
setup_namespace() {
  kubectl --kubeconfig="$KUBECONFIG" create namespace "$CHAOS_NAMESPACE" 2>/dev/null || true
}

# Clean up chaos namespace
cleanup_namespace() {
  kubectl --kubeconfig="$KUBECONFIG" delete namespace "$CHAOS_NAMESPACE" --wait=false 2>/dev/null || true
  # Wait for namespace to actually terminate
  for i in $(seq 1 30); do
    if ! kubectl --kubeconfig="$KUBECONFIG" get namespace "$CHAOS_NAMESPACE" 2>/dev/null; then
      break
    fi
    sleep 2
  done
}

# Execute an inject action
execute_inject() {
  local inject_json="$1"
  local inject_type=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['type'])")

  case "$inject_type" in
    configmap_patch)
      local ns=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['namespace'])")
      local name=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['name'])")
      local patch=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['patch'])")
      # Save original for restore
      kubectl --kubeconfig="$KUBECONFIG" -n "$ns" get configmap "$name" -o json > "/tmp/chaos-original-$name.json"
      kubectl --kubeconfig="$KUBECONFIG" -n "$ns" patch configmap "$name" -p "$patch"
      ;;
    scale)
      local ns=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['namespace'])")
      local resource=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['resource'])")
      local replicas=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['replicas'])")
      # Save original replica count
      kubectl --kubeconfig="$KUBECONFIG" -n "$ns" get "$resource" -o jsonpath='{.spec.replicas}' > "/tmp/chaos-original-replicas.txt"
      kubectl --kubeconfig="$KUBECONFIG" -n "$ns" scale "$resource" --replicas="$replicas"
      ;;
    deploy)
      local ns=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['namespace'])")
      local manifest=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['manifest'])")
      setup_namespace
      kubectl --kubeconfig="$KUBECONFIG" -n "$ns" apply -f "$MANIFESTS_DIR/$manifest"
      ;;
    deploy_then_delete_secret)
      local ns=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['namespace'])")
      local manifest=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['manifest'])")
      local secret=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['secret_name'])")
      local delay=$(echo "$inject_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['delay_seconds'])")
      setup_namespace
      kubectl --kubeconfig="$KUBECONFIG" -n "$ns" apply -f "$MANIFESTS_DIR/$manifest"
      sleep "$delay"
      kubectl --kubeconfig="$KUBECONFIG" -n "$ns" delete secret "$secret" --ignore-not-found
      ;;
  esac
}

# Wait for condition
wait_for_condition() {
  local wait_json="$1"
  local wait_type=$(echo "$wait_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['type'])")
  local timeout=$(echo "$wait_json" | python3 -c "import sys,json;print(json.load(sys.stdin).get('timeout_seconds', 120))")

  case "$wait_type" in
    pod_status)
      local ns=$(echo "$wait_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['namespace'])")
      local name_contains=$(echo "$wait_json" | python3 -c "import sys,json;print(json.load(sys.stdin).get('name_contains',''))")
      local target_status=$(echo "$wait_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['status'])")
      for i in $(seq 1 $((timeout / 5))); do
        local status=$(kubectl --kubeconfig="$KUBECONFIG" -n "$ns" get pods 2>/dev/null | grep "$name_contains" | awk '{print $3}' | head -1)
        if echo "$status" | grep -qE "$target_status"; then
          log "Condition met: pod status = $status"
          return 0
        fi
        sleep 5
      done
      warn "Timeout waiting for pod status $target_status"
      return 1
      ;;
    log_pattern)
      local pattern=$(echo "$wait_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['pattern'])")
      # Check savants daemon log and kubectl logs
      for i in $(seq 1 $((timeout / 5))); do
        if grep -qE "$pattern" ~/.savants/daemon.log 2>/dev/null; then
          log "Condition met: log pattern found"
          return 0
        fi
        sleep 5
      done
      warn "Timeout waiting for log pattern: $pattern"
      return 1
      ;;
    pod_gone)
      local ns=$(echo "$wait_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['namespace'])")
      local label=$(echo "$wait_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['label'])")
      for i in $(seq 1 $((timeout / 5))); do
        local count=$(kubectl --kubeconfig="$KUBECONFIG" -n "$ns" get pods -l "$label" --no-headers 2>/dev/null | wc -l)
        if [ "$count" -eq 0 ]; then
          log "Condition met: pods gone"
          return 0
        fi
        sleep 5
      done
      warn "Timeout waiting for pods to terminate"
      return 1
      ;;
    event_pattern)
      local ns=$(echo "$wait_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['namespace'])")
      local pattern=$(echo "$wait_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['pattern'])")
      for i in $(seq 1 $((timeout / 5))); do
        if kubectl --kubeconfig="$KUBECONFIG" -n "$ns" get events --no-headers 2>/dev/null | grep -qE "$pattern"; then
          log "Condition met: event pattern found"
          return 0
        fi
        sleep 5
      done
      warn "Timeout waiting for event pattern"
      return 1
      ;;
  esac
}

# Execute restore action
execute_restore() {
  local restore_json="$1"
  local restore_type=$(echo "$restore_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['type'])")

  case "$restore_type" in
    configmap_patch)
      local ns=$(echo "$restore_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['namespace'])")
      local name=$(echo "$restore_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['name'])")
      local patch=$(echo "$restore_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['patch'])")
      kubectl --kubeconfig="$KUBECONFIG" -n "$ns" patch configmap "$name" -p "$patch"
      ;;
    scale)
      local ns=$(echo "$restore_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['namespace'])")
      local resource=$(echo "$restore_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['resource'])")
      local replicas=$(echo "$restore_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['replicas'])")
      kubectl --kubeconfig="$KUBECONFIG" -n "$ns" scale "$resource" --replicas="$replicas"
      ;;
    delete_namespace)
      local ns=$(echo "$restore_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['namespace'])")
      cleanup_namespace
      ;;
  esac
}

# Run diagnosis and score
run_diagnosis_and_score() {
  local scenario_json="$1"
  local error_signal=$(echo "$scenario_json" | python3 -c "import sys,json;print(json.load(sys.stdin)['error_signal'])")

  # Run savants diagnose-error
  local diagnosis=$(echo "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"chaos-test\",\"version\":\"1.0\"}}}
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"diagnose-error\",\"arguments\":{\"error\":\"$error_signal\"}}}" | $SAVANTS_BIN serve 2>/dev/null | tail -1 | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('result',{}).get('content',[{}])[0].get('text',''))" 2>/dev/null)

  # Score the diagnosis
  python3 -c "
import json, sys

scenario = json.loads('''$scenario_json''')
diagnosis = '''$diagnosis'''
gt = scenario['ground_truth']

score = 0
max_score = 10
details = []

# Must-mention keywords (0-6 points)
must_mention = gt.get('must_mention', [])
mentioned = 0
for kw in must_mention:
    if kw.lower() in diagnosis.lower():
        mentioned += 1
if must_mention:
    keyword_score = round((mentioned / len(must_mention)) * 6)
    score += keyword_score
    details.append(f'Keywords: {mentioned}/{len(must_mention)} ({keyword_score}/6)')

# Category identification (0-2 points)
category = gt.get('must_identify_as', '')
if category and category.lower() in diagnosis.lower():
    score += 2
    details.append(f'Category: CORRECT ({category})')
else:
    details.append(f'Category: MISSED (expected {category})')

# Has a conclusion (0-2 points)
if 'CONCLUSION' in diagnosis or 'ROOT CAUSE' in diagnosis:
    score += 2
    details.append('Conclusion: present')
else:
    details.append('Conclusion: MISSING')

# Grade
if score >= 8: grade = 'CORRECT'
elif score >= 5: grade = 'PARTIAL'
elif score >= 3: grade = 'DIRECTION'
else: grade = 'WRONG'

result = {
    'score': score,
    'max_score': max_score,
    'grade': grade,
    'details': details,
    'diagnosis_length': len(diagnosis)
}
print(json.dumps(result))
"
}

# Main execution
main() {
  local scenarios_json=$(parse_scenarios)
  local total=$(echo "$scenarios_json" | python3 -c "import sys,json;print(len(json.load(sys.stdin)))")
  local passed=0
  local failed=0
  local partial=0
  local results="[]"

  log "Savants Chaos Test Runner"
  log "Scenarios to run: $total"
  log "Results file: $RESULTS_FILE"
  echo ""

  if $DRY_RUN; then
    log "DRY RUN - parsing scenarios only"
    echo "$scenarios_json" | python3 -c "
import sys, json
for s in json.load(sys.stdin):
    print(f'{s[\"id\"]:25s} {s[\"category\"]:20s} {s[\"difficulty\"]:8s} {s[\"name\"]}')
"
    exit 0
  fi

  # Force a fresh K8s snapshot in the daemon
  log "Triggering fresh K8s snapshot..."

  # Iterate scenarios
  echo "$scenarios_json" | python3 -c "
import sys, json
for s in json.load(sys.stdin):
    print(json.dumps(s))
" | while IFS= read -r scenario_line; do
    local sid=$(echo "$scenario_line" | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
    local sname=$(echo "$scenario_line" | python3 -c "import sys,json;print(json.load(sys.stdin)['name'])")
    local scat=$(echo "$scenario_line" | python3 -c "import sys,json;print(json.load(sys.stdin)['category'])")

    echo ""
    log "${BOLD}[$sid] $sname${RESET}"
    log "Category: $scat"

    # 1. Inject fault
    log "Injecting fault..."
    local inject_json=$(echo "$scenario_line" | python3 -c "import sys,json;print(json.dumps(json.load(sys.stdin)['inject']))")
    if ! execute_inject "$inject_json"; then
      fail "$sid: Injection failed"
      continue
    fi

    # 2. Wait for condition
    log "Waiting for fault detection..."
    local wait_json=$(echo "$scenario_line" | python3 -c "import sys,json;print(json.dumps(json.load(sys.stdin)['wait_for']))")
    execute_inject_result=true
    if ! wait_for_condition "$wait_json"; then
      warn "$sid: Condition not met within timeout (testing diagnosis anyway)"
    fi

    # 3. Wait for Savants daemon to ingest (give it one cycle)
    log "Waiting for Savants to process..."
    sleep 15

    # 4. Run diagnosis and score
    log "Running diagnosis..."
    local score_result=$(run_diagnosis_and_score "$scenario_line" 2>/dev/null || echo '{"grade":"ERROR","score":0,"details":["diagnosis failed"]}')
    local grade=$(echo "$score_result" | python3 -c "import sys,json;print(json.load(sys.stdin)['grade'])")
    local score=$(echo "$score_result" | python3 -c "import sys,json;print(json.load(sys.stdin)['score'])")

    case "$grade" in
      CORRECT) pass "$sid: $sname (score: $score/10)"; passed=$((passed + 1)) ;;
      PARTIAL) warn "$sid: $sname (score: $score/10 - PARTIAL)"; partial=$((partial + 1)) ;;
      *) fail "$sid: $sname (score: $score/10 - $grade)"; failed=$((failed + 1)) ;;
    esac

    # 5. Restore
    log "Restoring..."
    local restore_json=$(echo "$scenario_line" | python3 -c "import sys,json;print(json.dumps(json.load(sys.stdin)['restore']))")
    execute_restore "$restore_json" || warn "Restore may have failed for $sid"

    # Wait for cluster to stabilize before next test
    sleep 10
  done

  # Summary
  echo ""
  echo "================================================================"
  echo "CHAOS TEST RESULTS"
  echo "================================================================"
  echo "Total:   $total"
  echo "Correct: $passed"
  echo "Partial: $partial"
  echo "Failed:  $failed"
  echo ""
  local accuracy=0
  if [ "$total" -gt 0 ]; then
    accuracy=$((passed * 100 / total))
  fi
  echo "Strict accuracy: ${accuracy}%"
  echo ""

  if [ "$accuracy" -ge 93 ]; then
    pass "Accuracy target met (>= 93%)"
    exit 0
  else
    fail "Accuracy below target (${accuracy}% < 93%)"
    exit 1
  fi
}

main "$@"
