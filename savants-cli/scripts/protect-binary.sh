#!/bin/sh
# Post-build binary protection for Savants.
#
# XOR-encrypts all Cypher query strings in the compiled binary's .rodata
# section. The runtime decryption happens in the graph client layer.
#
# Usage: ./scripts/protect-binary.sh target/release/savants
#
# What this does:
# 1. Extracts strings matching Cypher patterns (MATCH, MERGE, CREATE INDEX)
# 2. XOR-encrypts them in-place in the binary
# 3. The graph.rs query() function XOR-decrypts before sending to Redis
#
# What this prevents:
# - `strings binary | grep MATCH` → shows gibberish
# - Casual competitors reading the graph schema
#
# What this does NOT prevent:
# - Debugger-attached runtime inspection
# - Network traffic capture (queries are sent in cleartext to Redis)

set -e

BINARY="${1:-target/release/savants}"
KEY="s4v4nts_m3m0ry_3ng1n3_2026_x9k2"

if [ ! -f "$BINARY" ]; then
    echo "Binary not found: $BINARY"
    exit 1
fi

echo "Protecting binary: $BINARY"

# Count Cypher queries before protection
BEFORE=$(strings "$BINARY" | grep -c -E "MATCH|MERGE|CREATE INDEX|DETACH DELETE" || true)
echo "  Cypher query strings found: $BEFORE"

if [ "$BEFORE" -eq 0 ]; then
    echo "  No queries to protect (already protected?)"
    exit 0
fi

# Create a Python script for the XOR patching
# (Using Python because binary patching in pure shell is painful)
python3 -c "
import sys, re

key = b'$KEY'
key_len = len(key)

with open('$BINARY', 'rb') as f:
    data = bytearray(f.read())

# Find all Cypher query patterns in the binary
patterns = [
    rb'MATCH \(',
    rb'MERGE \(',
    rb'CREATE INDEX',
    rb'DETACH DELETE',
    rb'RETURN ',
    rb'WHERE ',
    rb'ORDER BY ',
    rb'SET [a-z]',
]

patched = 0
# Find runs of printable ASCII that contain Cypher keywords
i = 0
while i < len(data) - 10:
    # Look for MATCH or MERGE start
    if (data[i:i+6] == b'MATCH ' or data[i:i+6] == b'MERGE '):
        # Find the extent of this string (printable ASCII run)
        start = i
        end = i
        while end < len(data) and 32 <= data[end] <= 126:
            end += 1
        query_len = end - start
        if query_len > 20:  # Only encrypt substantial queries
            for j in range(start, end):
                data[j] ^= key[(j - start) % key_len]
            patched += 1
            i = end
            continue
    i += 1

with open('$BINARY', 'wb') as f:
    f.write(data)

print(f'  Encrypted {patched} query regions')
"

AFTER=$(strings "$BINARY" | grep -c -E "MATCH|MERGE|CREATE INDEX|DETACH DELETE" || true)
echo "  Cypher queries after protection: $AFTER (should be ~0)"
echo "  Done."
