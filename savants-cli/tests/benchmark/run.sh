#!/bin/bash
# Savants Accuracy Benchmark Runner
# Runs all test cases through diagnose-error and scores accuracy.
#
# Usage:
#   ./run.sh                    # Run development set
#   ./run.sh --set holdout      # Run holdout set (release only)
#   ./run.sh --set chaos        # Run chaos tests only
#   ./run.sh --category infra   # Run one category
#   ./run.sh --verbose          # Show full diagnosis output

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RESULTS_DIR="$SCRIPT_DIR/results"
SAVANTS_BIN="${SAVANTS_BIN:-savants}"
VERBOSE=false
TEST_SET="development"
CATEGORY=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --set) TEST_SET="$2"; shift 2 ;;
    --category) CATEGORY="$2"; shift 2 ;;
    --verbose) VERBOSE=true; shift ;;
    *) echo "Unknown: $1"; exit 1 ;;
  esac
done

mkdir -p "$RESULTS_DIR"

# Select case directories based on test set
if [ "$TEST_SET" = "holdout" ]; then
  CASE_DIRS="$SCRIPT_DIR/holdout"
elif [ "$TEST_SET" = "chaos" ]; then
  CASE_DIRS="$SCRIPT_DIR/cases/chaos"
else
  CASE_DIRS="$SCRIPT_DIR/cases/github-bugs $SCRIPT_DIR/cases/postmortems $SCRIPT_DIR/cases/production $SCRIPT_DIR/cases/regression $SCRIPT_DIR/cases/oss"
fi

# Colors
if [ -t 1 ]; then
  G='\033[32m'; R='\033[31m'; Y='\033[33m'; C='\033[36m'; B='\033[1m'; N='\033[0m'
else
  G=''; R=''; Y=''; C=''; B=''; N=''
fi

echo -e "${B}Savants Accuracy Benchmark${N}"
echo "Test set: $TEST_SET"
echo "Date: $(date -Iseconds)"
echo ""

# Collect all JSON case files
CASES=()
for dir in $CASE_DIRS; do
  if [ -d "$dir" ]; then
    for f in "$dir"/*.json; do
      [ -f "$f" ] || continue
      if [ -n "$CATEGORY" ]; then
        cat_match=$(python3 -c "import json;print(json.load(open('$f')).get('category',''))" 2>/dev/null)
        [ "$cat_match" = "$CATEGORY" ] || continue
      fi
      CASES+=("$f")
    done
  fi
done

TOTAL=${#CASES[@]}
if [ "$TOTAL" -eq 0 ]; then
  echo "No test cases found. Run harvest scripts first:"
  echo "  ./harvest/github_bugs.sh"
  echo "  ./chaos-harness/runner.sh --all"
  exit 1
fi

echo "Cases: $TOTAL"
echo ""

# Results tracking
CORRECT=0
PARTIAL=0
DIRECTION=0
WRONG=0
ERRORS=0
RESULTS_JSON="[]"
START_TIME=$(date +%s)

for case_file in "${CASES[@]}"; do
  case_id=$(python3 -c "import json;print(json.load(open('$case_file'))['id'])")
  case_title=$(python3 -c "import json;print(json.load(open('$case_file')).get('title','')[:60])")
  error_msg=$(python3 -c "import json;print(json.load(open('$case_file'))['error_signal']['message'])" | sed 's/"/\\"/g' | head -c 200)
  repo=$(python3 -c "import json;print(json.load(open('$case_file')).get('metadata',{}).get('repo','talent-pipeline').split('/')[-1])" 2>/dev/null || echo "unknown")

  # Run diagnose-error
  diagnosis=$(echo "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"benchmark\",\"version\":\"1.0\"}}}
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"diagnose-error\",\"arguments\":{\"error\":\"$error_msg\",\"repo\":\"$repo\"}}}" | timeout 30 $SAVANTS_BIN serve 2>/dev/null | tail -1 | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('result',{}).get('content',[{}])[0].get('text',''))" 2>/dev/null || echo "TIMEOUT")

  # Score
  score_result=$(python3 -c "
import json, sys

case_data = json.load(open('$case_file'))
diagnosis = '''$diagnosis'''
gt = case_data['ground_truth']
es = case_data.get('expected_signals', {})

score = 0
details = []

# Root cause file (0-3)
rc_file = gt.get('root_cause_file', '')
if rc_file and rc_file in diagnosis:
    score += 3
    details.append('file:MATCH')
elif rc_file:
    parts = [p for p in rc_file.split('/') if len(p) > 3]
    if any(p in diagnosis for p in parts):
        score += 1
        details.append('file:PARTIAL')
    else:
        details.append('file:MISS')

# Keywords (0-4)
must_mention = es.get('must_mention', [])
if must_mention:
    hits = sum(1 for kw in must_mention if kw.lower() in diagnosis.lower())
    kw_score = round((hits / len(must_mention)) * 4)
    score += kw_score
    details.append(f'keywords:{hits}/{len(must_mention)}')

# Category (0-2)
cat = es.get('must_identify_category', gt.get('root_cause_category', ''))
if cat and cat.lower() in diagnosis.lower():
    score += 2
    details.append('category:MATCH')
else:
    details.append('category:MISS')

# Has conclusion (0-1)
if 'CONCLUSION' in diagnosis or 'ROOT CAUSE' in diagnosis:
    score += 1
    details.append('conclusion:YES')

# Grade
if score >= 8: grade = 'CORRECT'
elif score >= 5: grade = 'PARTIAL'
elif score >= 3: grade = 'DIRECTION'
else: grade = 'WRONG'

print(json.dumps({'grade': grade, 'score': score, 'details': details}))
" 2>/dev/null || echo '{"grade":"ERROR","score":0,"details":["scoring failed"]}')

  grade=$(echo "$score_result" | python3 -c "import sys,json;print(json.load(sys.stdin)['grade'])")
  score=$(echo "$score_result" | python3 -c "import sys,json;print(json.load(sys.stdin)['score'])")

  case "$grade" in
    CORRECT)  echo -e "  ${G}PASS${N} [$score/10] $case_id: $case_title"; CORRECT=$((CORRECT+1)) ;;
    PARTIAL)  echo -e "  ${Y}PART${N} [$score/10] $case_id: $case_title"; PARTIAL=$((PARTIAL+1)) ;;
    DIRECTION) echo -e "  ${Y}DIR ${N} [$score/10] $case_id: $case_title"; DIRECTION=$((DIRECTION+1)) ;;
    WRONG)    echo -e "  ${R}FAIL${N} [$score/10] $case_id: $case_title"; WRONG=$((WRONG+1)) ;;
    ERROR)    echo -e "  ${R}ERR ${N} [$score/10] $case_id: $case_title"; ERRORS=$((ERRORS+1)) ;;
  esac

  if $VERBOSE; then
    echo "    $(echo "$score_result" | python3 -c "import sys,json;print(' | '.join(json.load(sys.stdin)['details']))")"
  fi
done

END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

echo ""
echo "================================================================"
echo "BENCHMARK RESULTS"
echo "================================================================"
echo "Test set:    $TEST_SET"
echo "Total cases: $TOTAL"
echo "Duration:    ${DURATION}s"
echo ""
echo "  CORRECT:   $CORRECT"
echo "  PARTIAL:   $PARTIAL"
echo "  DIRECTION: $DIRECTION"
echo "  WRONG:     $WRONG"
echo "  ERROR:     $ERRORS"
echo ""

# Calculate accuracies
if [ "$TOTAL" -gt 0 ]; then
  STRICT=$((CORRECT * 100 / TOTAL))
  USEFUL=$(( (CORRECT + PARTIAL) * 100 / TOTAL ))
  CATEGORY_ACC=$(( (CORRECT + PARTIAL + DIRECTION) * 100 / TOTAL ))
else
  STRICT=0; USEFUL=0; CATEGORY_ACC=0
fi

echo "Strict accuracy (CORRECT only):        ${STRICT}%"
echo "Useful accuracy (CORRECT + PARTIAL):    ${USEFUL}%"
echo "Category accuracy (+ DIRECTION):        ${CATEGORY_ACC}%"
echo ""

# Save results
RESULTS_FILE="$RESULTS_DIR/benchmark-${TEST_SET}-$(date +%Y%m%d-%H%M%S).json"
python3 -c "
import json
results = {
    'test_set': '$TEST_SET',
    'date': '$(date -Iseconds)',
    'total': $TOTAL,
    'correct': $CORRECT,
    'partial': $PARTIAL,
    'direction': $DIRECTION,
    'wrong': $WRONG,
    'errors': $ERRORS,
    'strict_accuracy': $STRICT,
    'useful_accuracy': $USEFUL,
    'category_accuracy': $CATEGORY_ACC,
    'duration_seconds': $DURATION
}
with open('$RESULTS_FILE', 'w') as f:
    json.dump(results, f, indent=2)
print(f'Results saved: $RESULTS_FILE')
"

# Also save the headline number for CI badges
echo "$STRICT" > "$RESULTS_DIR/${TEST_SET}-accuracy.txt"

# Exit code based on target
if [ "$TEST_SET" = "holdout" ]; then
  TARGET=95
else
  TARGET=93
fi

echo ""
if [ "$STRICT" -ge "$TARGET" ]; then
  echo -e "${G}${B}PASS: Accuracy ${STRICT}% >= ${TARGET}% target${N}"
  exit 0
else
  echo -e "${R}${B}FAIL: Accuracy ${STRICT}% < ${TARGET}% target${N}"
  exit 1
fi
