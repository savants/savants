#!/bin/bash
# GitHub Bug-Fix Pair Harvester
# Finds closed bug issues with linked merged PRs from popular repos.
# Each pair becomes a benchmark test case for diagnose-error.
#
# Usage: ./github_bugs.sh [--limit 30] [--repo vercel/next.js]
# Requires: gh CLI authenticated

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CASES_DIR="$SCRIPT_DIR/../cases/github-bugs"
FIXTURES_DIR="$SCRIPT_DIR/../fixtures"
LIMIT="${1:-30}"

mkdir -p "$CASES_DIR" "$FIXTURES_DIR"

# Target repos: popular TypeScript/JavaScript with good bug labeling
REPOS=(
  "vercel/next.js"
  "prisma/prisma"
  "trpc/trpc"
  "honojs/hono"
  "calcom/cal.com"
  "remix-run/remix"
  "drizzle-team/drizzle-orm"
  "nestjs/nest"
  "vitejs/vite"
  "supabase/supabase"
)

echo "GitHub Bug-Fix Pair Harvester"
echo "Target: $LIMIT cases per repo"
echo ""

total_harvested=0

for repo in "${REPOS[@]}"; do
  repo_short=$(echo "$repo" | tr '/' '-')
  echo "=== $repo ==="

  # Get closed bug issues with linked PRs
  issues=$(gh issue list --repo "$repo" --label "bug" --state closed --limit "$LIMIT" \
    --json number,title,body,closedAt,labels \
    2>/dev/null || echo "[]")

  count=$(echo "$issues" | python3 -c "import sys,json;print(len(json.load(sys.stdin)))" 2>/dev/null || echo "0")
  echo "  Found $count closed bug issues"

  echo "$issues" | python3 -c "
import sys, json, subprocess, os

issues = json.load(sys.stdin)
repo = '$repo'
repo_short = '$repo_short'
cases_dir = '$CASES_DIR'
fixtures_dir = '$FIXTURES_DIR'
harvested = 0

for issue in issues:
    num = issue['number']
    title = issue.get('title', '')
    body = (issue.get('body') or '')[:1000]

    # Find linked merged PRs for this issue
    # gh searches PR body for 'fixes #N' or 'closes #N'
    try:
        result = subprocess.run(
            ['gh', 'pr', 'list', '--repo', repo, '--state', 'merged',
             '--search', f'fixes #{num} OR closes #{num} OR resolves #{num}',
             '--limit', '1', '--json', 'number,title,files,headRefName,mergedAt,author'],
            capture_output=True, text=True, timeout=15
        )
        prs = json.loads(result.stdout) if result.returncode == 0 else []
    except:
        prs = []

    if not prs:
        continue

    pr = prs[0]
    pr_num = pr['number']
    pr_files = pr.get('files', [])
    pr_branch = pr.get('headRefName', '')
    pr_author = (pr.get('author') or {}).get('login', '')

    if not pr_files or len(pr_files) > 10:
        continue  # Skip if no files or too many (complex fix)

    # Extract root cause from first changed file
    root_file = pr_files[0].get('path', '') if pr_files else ''
    root_additions = pr_files[0].get('additions', 0) if pr_files else 0
    root_deletions = pr_files[0].get('deletions', 0) if pr_files else 0

    if not root_file:
        continue

    # Determine category from file path
    category = 'application'
    if any(x in root_file.lower() for x in ['config', 'env', '.yaml', '.yml', '.toml']):
        category = 'configuration'
    elif any(x in root_file.lower() for x in ['test', 'spec', '__test']):
        continue  # Skip test-only fixes
    elif any(x in root_file.lower() for x in ['docker', 'k8s', 'helm', 'deploy', 'infra']):
        category = 'infrastructure'

    # Build error signal from issue title + body snippet
    error_signal = title
    if body:
        # Extract first code block or error message from body
        lines = body.split('\n')
        for line in lines:
            if 'error' in line.lower() or 'typeerror' in line.lower() or 'failed' in line.lower():
                error_signal = line.strip()[:200]
                break

    # Determine difficulty
    difficulty = 'easy'
    if len(pr_files) > 3:
        difficulty = 'hard'
    elif len(pr_files) > 1:
        difficulty = 'medium'

    # Build must_mention keywords from the file path
    must_mention = []
    parts = root_file.replace('/', ' ').replace('.', ' ').replace('-', ' ').replace('_', ' ').split()
    must_mention = [p for p in parts if len(p) > 3 and p not in ['src', 'lib', 'utils', 'index', 'test']][:4]

    case = {
        'id': f'gh-{repo_short}-{num}',
        'source': 'github-bug',
        'title': f'{repo}#{num}: {title}',
        'category': category,
        'difficulty': difficulty,
        'error_signal': {
            'message': error_signal,
            'source': 'github_issue',
            'issue_url': f'https://github.com/{repo}/issues/{num}'
        },
        'ground_truth': {
            'root_cause_file': root_file,
            'root_cause_function': None,
            'root_cause_description': f'Fixed in PR #{pr_num} by @{pr_author}',
            'root_cause_category': category,
            'fix_pr': f'#{pr_num}',
            'fix_branch': pr_branch,
            'files_changed': len(pr_files),
            'additions': root_additions,
            'deletions': root_deletions
        },
        'expected_signals': {
            'must_mention': must_mention,
            'must_not_mention': [],
            'must_identify_category': category
        },
        'metadata': {
            'repo': repo,
            'issue_number': num,
            'pr_number': pr_num,
            'harvested_date': '$(date +%Y-%m-%d)',
            'confidence': 'medium'
        }
    }

    case_file = os.path.join(cases_dir, f'gh-{repo_short}-{num}.json')
    with open(case_file, 'w') as f:
        json.dump(case, f, indent=2)

    harvested += 1
    print(f'  [{harvested}] #{num}: {title[:60]}... -> PR #{pr_num} ({len(pr_files)} files)')

print(f'  Harvested: {harvested} cases from {repo}')
" 2>&1

  repo_count=$(ls "$CASES_DIR"/gh-${repo_short}-*.json 2>/dev/null | wc -l)
  total_harvested=$((total_harvested + repo_count))
  echo ""
done

echo "================================================================"
echo "Total harvested: $total_harvested cases"
echo "Cases directory: $CASES_DIR"
echo "================================================================"
