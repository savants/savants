#!/bin/bash
# Analyze missed Savants opportunities from Claude Code sessions.
# Reads the log created by detect-missed-savants.sh and shows a summary.
#
# Usage: .claude/hooks/savants-audit.sh

LOG_FILE="${CLAUDE_PROJECT_DIR:-.}/.claude/savants-misses.jsonl"

if [ ! -f "$LOG_FILE" ]; then
  echo "No missed opportunities logged yet."
  echo "The hook will start logging when Claude Code uses Grep/Read instead of Savants."
  exit 0
fi

TOTAL=$(wc -l < "$LOG_FILE")
GREP_COUNT=$(grep -c '"tool":"Grep"' "$LOG_FILE")
READ_COUNT=$(grep -c '"tool":"Read"' "$LOG_FILE")
GLOB_COUNT=$(grep -c '"tool":"Glob"' "$LOG_FILE")

# Estimate tokens wasted (avg 2500 tokens per unnecessary file read)
TOKENS_WASTED=$((READ_COUNT * 2500 + GREP_COUNT * 500))
COST_WASTED=$(python3 -c "print(f'${TOKENS_WASTED * 3.0 / 1000000:.4f}')" 2>/dev/null)

echo "=== Savants Audit ==="
echo ""
echo "Missed opportunities: $TOTAL"
echo "  Grep instead of where_used/search_code: $GREP_COUNT"
echo "  Read instead of file_skeleton/module_exports: $READ_COUNT"
echo "  Glob instead of search_code: $GLOB_COUNT"
echo ""
echo "Estimated tokens wasted: ~$TOKENS_WASTED"
echo "Estimated cost wasted: ~\$$COST_WASTED"
echo ""
echo "Most common patterns:"
python3 -c "
import json, sys
from collections import Counter

patterns = Counter()
with open('$LOG_FILE') as f:
    for line in f:
        try:
            d = json.loads(line)
            alt = d.get('savants_alternative', '')
            patterns[alt] += 1
        except:
            pass

for pattern, count in patterns.most_common(5):
    print(f'  {count:>4}x  Could have used: {pattern}')
" 2>/dev/null

echo ""
echo "Recent misses:"
tail -5 "$LOG_FILE" | python3 -c "
import sys, json
for line in sys.stdin:
    try:
        d = json.loads(line)
        tool = d.get('tool', '?')
        alt = d.get('savants_alternative', '?')
        ts = d.get('timestamp', '?')[:19]
        print(f'  {ts} | {tool} -> should use {alt}')
    except:
        pass
" 2>/dev/null
