#!/bin/sh
# D1 Migration Runner
#
# Usage:
#   ./scripts/migrate.sh                    # apply all pending migrations to production
#   ./scripts/migrate.sh --staging          # apply to staging database
#   ./scripts/migrate.sh --dry-run          # show what would be applied
#   ./scripts/migrate.sh --status           # show migration status
#
# Migrations are applied in order. Each migration is idempotent
# (uses IF NOT EXISTS / ON CONFLICT DO NOTHING).
# The _migrations table tracks what's been applied.

set -e

DB_NAME="savants-cloud"
ACCOUNT_ID="4992fd600f9894326a82a0f8573a7c38"
MIGRATIONS_DIR="$(dirname "$0")/../migrations"
DRY_RUN=false
SHOW_STATUS=false

# Parse args
for arg in "$@"; do
  case "$arg" in
    --staging) DB_NAME="savants-cloud-staging" ;;
    --dry-run) DRY_RUN=true ;;
    --status) SHOW_STATUS=true ;;
  esac
done

if [ -z "$CF_API_TOKEN" ] && [ -z "$CLOUDFLARE_API_TOKEN" ]; then
  echo "Error: CF_API_TOKEN or CLOUDFLARE_API_TOKEN must be set"
  exit 1
fi
TOKEN="${CF_API_TOKEN:-$CLOUDFLARE_API_TOKEN}"

# Get database ID
DB_ID=$(curl -sf "https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/d1/database" \
  -H "Authorization: Bearer $TOKEN" | \
  python3 -c "import sys,json; dbs=json.load(sys.stdin)['result']; print(next((d['uuid'] for d in dbs if d['name']=='$DB_NAME'),''))" 2>/dev/null)

if [ -z "$DB_ID" ]; then
  echo "Error: Database '$DB_NAME' not found"
  exit 1
fi

API="https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/d1/database/$DB_ID/query"

run_sql() {
  curl -sf -X POST "$API" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    -d "{\"sql\":$(python3 -c "import json,sys; print(json.dumps(sys.stdin.read()))" <<< "$1")}" 2>/dev/null
}

# Ensure _migrations table exists
run_sql "CREATE TABLE IF NOT EXISTS _migrations (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE, applied_at INTEGER NOT NULL DEFAULT (unixepoch()))" > /dev/null

# Get applied migrations
APPLIED=$(run_sql "SELECT name FROM _migrations ORDER BY id" | \
  python3 -c "import sys,json; d=json.load(sys.stdin); [print(r['name']) for r in d.get('result',[{}])[0].get('results',[])]" 2>/dev/null)

if [ "$SHOW_STATUS" = true ]; then
  echo "Database: $DB_NAME ($DB_ID)"
  echo ""
  echo "Applied migrations:"
  if [ -z "$APPLIED" ]; then
    echo "  (none)"
  else
    echo "$APPLIED" | while read -r name; do echo "  ✓ $name"; done
  fi
  echo ""
  echo "Pending migrations:"
  PENDING=false
  for file in "$MIGRATIONS_DIR"/*.sql; do
    name=$(basename "$file" .sql)
    if ! echo "$APPLIED" | grep -q "^${name}$"; then
      echo "  ○ $name"
      PENDING=true
    fi
  done
  if [ "$PENDING" = false ]; then
    echo "  (none - all up to date)"
  fi
  exit 0
fi

# Apply pending migrations
APPLIED_COUNT=0
for file in $(ls "$MIGRATIONS_DIR"/*.sql | sort); do
  name=$(basename "$file" .sql)

  # Skip if already applied
  if echo "$APPLIED" | grep -q "^${name}$"; then
    continue
  fi

  if [ "$DRY_RUN" = true ]; then
    echo "[dry-run] Would apply: $name"
    continue
  fi

  echo "Applying: $name..."

  # Read and execute the migration as individual statements
  while IFS= read -r stmt || [ -n "$stmt" ]; do
    stmt=$(echo "$stmt" | sed 's/--.*$//' | tr '\n' ' ' | xargs)
    if [ -n "$stmt" ]; then
      RESULT=$(run_sql "$stmt")
      if echo "$RESULT" | python3 -c "import sys,json; d=json.load(sys.stdin); exit(0 if d.get('success') else 1)" 2>/dev/null; then
        :
      else
        echo "  ERROR in $name: $stmt"
        echo "  $RESULT"
        exit 1
      fi
    fi
  done < <(python3 -c "
import sys
sql = open('$file').read()
# Split on semicolons but not within strings
stmts = []
current = []
for line in sql.split('\n'):
    stripped = line.strip()
    if stripped.startswith('--'):
        continue
    current.append(line)
    if ';' in line:
        stmt = ' '.join(current).strip()
        if stmt and stmt != ';':
            print(stmt)
        current = []
")

  echo "  ✓ $name applied"
  APPLIED_COUNT=$((APPLIED_COUNT + 1))
done

if [ "$DRY_RUN" = true ]; then
  echo ""
  echo "Dry run complete. No changes made."
elif [ "$APPLIED_COUNT" -eq 0 ]; then
  echo "All migrations already applied."
else
  echo ""
  echo "$APPLIED_COUNT migration(s) applied successfully."
fi
