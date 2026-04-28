#!/bin/sh
# Fast pre-push checks (<15 seconds)
# Install: ln -sf ../../scripts/pre-push.sh .git/hooks/pre-push
#
# Runs: type check + unit tests + contract smoke test
# Skips: full E2E (that runs in CI)

set -e

if [ -t 1 ]; then
    G='\033[32m'; R='\033[31m'; B='\033[1m'; D='\033[2m'; X='\033[0m'
else
    G=''; R=''; B=''; D=''; X=''
fi

step() { printf "${B}[%s]${X} %s... " "$1" "$2"; }
ok() { printf "${G}ok${X} ${D}(%ss)${X}\n" "$1"; }
err() { printf "${R}FAIL${X}\n"; exit 1; }

START=$(date +%s)

# 1. TypeScript type check (~3s)
step "1/4" "TypeScript types"
T=$(date +%s)
(cd workers/api && npx tsc --noEmit 2>/dev/null) && ok "$(($(date +%s) - T))" || err

# 2. Rust check (~5s with cache)
step "2/4" "Rust check"
T=$(date +%s)
(cd savants-cli && cargo check 2>/dev/null) && ok "$(($(date +%s) - T))" || err

# 3. Rust unit tests (~2s)
step "3/4" "Rust unit tests"
T=$(date +%s)
(cd savants-cli && cargo test 2>/dev/null) && ok "$(($(date +%s) - T))" || err

# 4. API smoke test (~3s)
step "4/4" "API smoke"
T=$(date +%s)
HEALTH=$(curl -sf --max-time 5 https://api.savants.cloud/health 2>/dev/null)
echo "$HEALTH" | grep -q '"ok"' && ok "$(($(date +%s) - T))" || err

TOTAL=$(($(date +%s) - START))
printf "\n${G}${B}All checks passed${X} ${D}(${TOTAL}s total)${X}\n"
