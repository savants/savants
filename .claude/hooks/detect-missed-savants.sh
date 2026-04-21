#!/bin/bash
# Detect when Claude Code uses Grep/Read instead of Savants MCP tools.
# Logs every occurrence for analysis. Helps identify when the LLM
# is burning tokens on file reading that Savants could answer instantly.

LOG_FILE="${CLAUDE_PROJECT_DIR:-.}/.claude/savants-misses.jsonl"
INPUT=$(cat)

TOOL_NAME=$(echo "$INPUT" | python3 -c "import sys,json;print(json.loads(sys.stdin.read()).get('tool_name',''))" 2>/dev/null)
TOOL_INPUT=$(echo "$INPUT" | python3 -c "import sys,json;print(json.dumps(json.loads(sys.stdin.read()).get('tool_input',{})))" 2>/dev/null)
SESSION_ID=$(echo "$INPUT" | python3 -c "import sys,json;print(json.loads(sys.stdin.read()).get('session_id',''))" 2>/dev/null)
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

# Detect patterns that Savants could handle
SAVANTS_ALTERNATIVE=""
case "$TOOL_NAME" in
  Grep)
    PATTERN=$(echo "$TOOL_INPUT" | python3 -c "import sys,json;print(json.loads(sys.stdin.read()).get('pattern',''))" 2>/dev/null)
    # If grepping for a function/class name, savants where_used or search_code is faster
    if echo "$PATTERN" | grep -qE '^[a-zA-Z_][a-zA-Z0-9_]*$'; then
      SAVANTS_ALTERNATIVE="where_used or search_code"
    fi
    ;;
  Read)
    FILE=$(echo "$TOOL_INPUT" | python3 -c "import sys,json;print(json.loads(sys.stdin.read()).get('file_path',''))" 2>/dev/null)
    LIMIT=$(echo "$TOOL_INPUT" | python3 -c "import sys,json;print(json.loads(sys.stdin.read()).get('limit',0))" 2>/dev/null)
    # If reading a full file (no offset/limit), file_skeleton would be cheaper
    if [ "$LIMIT" = "0" ] || [ "$LIMIT" = "None" ] || [ -z "$LIMIT" ]; then
      SAVANTS_ALTERNATIVE="file_skeleton or module_exports"
    fi
    ;;
  Glob)
    SAVANTS_ALTERNATIVE="search_code (if searching for code symbols)"
    ;;
esac

if [ -n "$SAVANTS_ALTERNATIVE" ]; then
  echo "{\"timestamp\":\"$TIMESTAMP\",\"session\":\"$SESSION_ID\",\"tool\":\"$TOOL_NAME\",\"input\":$TOOL_INPUT,\"savants_alternative\":\"$SAVANTS_ALTERNATIVE\"}" >> "$LOG_FILE"
fi

exit 0
