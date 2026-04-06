#!/usr/bin/env bash
# Install the SynapCode post-merge git hook into a repository.
#
# Usage: ./install-hooks.sh /path/to/repo

set -euo pipefail

REPO_PATH="${1:?Usage: install-hooks.sh <repo-path>}"
HOOK_DIR="${REPO_PATH}/.git/hooks"
HOOK_FILE="${HOOK_DIR}/post-merge"

if [ ! -d "${REPO_PATH}/.git" ]; then
    echo "Error: ${REPO_PATH} is not a git repository"
    exit 1
fi

mkdir -p "${HOOK_DIR}"

cat > "${HOOK_FILE}" << 'HOOK'
#!/usr/bin/env bash
# SynapCode post-merge hook: triggers incremental graph sync via Temporal.
# Installed by: scripts/install-hooks.sh

echo "[SynapCode] Post-merge hook triggered, starting incremental sync..."
python -m synapcode.sync.git_hooks 2>&1 | while read -r line; do
    echo "[SynapCode] ${line}"
done
HOOK

chmod +x "${HOOK_FILE}"

# Create the bookmark directory
mkdir -p "${REPO_PATH}/.synapcode"

echo "SynapCode post-merge hook installed at ${HOOK_FILE}"
echo "Run a full index first, then the hook will handle incremental updates."
