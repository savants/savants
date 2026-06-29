# Savants PR Analysis - GitHub Action

Automatic code risk analysis on every pull request. Posts a comment with blast radius, guard violations, and a composite risk score.

## Quick Start

Copy the workflow file to your repository:

```bash
mkdir -p .github/workflows
curl -fsSL https://raw.githubusercontent.com/savants/savants/main/ci/savants-pr-check.yml \
  > .github/workflows/savants-pr-check.yml
git add .github/workflows/savants-pr-check.yml
git commit -m "Add Savants PR analysis"
```

That's it. The next PR will get an automatic analysis comment.

## What It Does

On every PR (opened, updated, reopened), the action:

1. **Installs savants** via `curl -fsSL savants.sh | sh` (cached binary, ~5s)
2. **Indexes the repo** with tree-sitter to build a code graph (functions, calls, imports)
3. **Identifies changed functions** from the PR diff
4. **Computes blast radius** for each changed function (how many callers, how many files affected)
5. **Checks guard rules** against the diff (dangerous patterns, removed safety checks, secrets)
6. **Posts a PR comment** with the full analysis
7. **Fails the check** if the risk score exceeds the configured threshold

## Example Comment

```
## Savants PR Analysis

**Risk Score: Medium (45/100)**

### Changed Functions
| Function | File | Callers | Blast Radius |
|----------|------|---------|--------------|
| processPayment | payment.ts:42 | 12 | 34 files |
| validateToken | auth.ts:89 | 7 | 18 files |

### Guard Violations
- src/config.ts: Modifies a sensitive file (credentials/secrets)
- 3 new TODO/FIXME/HACK comments added

### Summary
- 2 functions changed, 52 downstream files affected
- 8 files modified, ~340 lines changed
- 1 guard warning(s), 0 guard block(s)

---
Powered by Savants — the code intelligence layer for engineering teams
```

## Configuration

All configuration is optional. Set these as GitHub Actions variables (Settings > Variables > Actions):

| Variable | Default | Description |
|----------|---------|-------------|
| `SAVANTS_RISK_THRESHOLD` | `75` | Fail the check if risk score >= this value. Set to `0` to disable. |
| `SAVANTS_MAX_DEPTH` | `3` | How deep to traverse the call graph for blast radius. |
| `SAVANTS_GUARD_PROFILE` | `standard` | Which guard profile to use for violation checks. |

## Guard Rules Checked

The built-in checks detect:

- **Force push**: `git push --force` without `--force-with-lease`
- **Credential files**: Changes to `.env`, `.pem`, `.key`, `credentials`, `secret` files
- **Debug statements**: `console.log`, `print()`, `debugger`, `binding.pry` left in code
- **TODO/FIXME**: New technical debt markers added
- **Large changes**: Files with 500+ lines added (suggests splitting)
- **Removed safety checks**: Deleted null guards, error handlers, catch blocks

If savants guard profiles are configured in the repo, those rules run too.

## Permissions

The workflow needs:

- `contents: read` — to check out and analyze the code
- `pull-requests: write` — to post/update the analysis comment

These are set in the workflow file. No additional tokens or secrets required beyond the default `GITHUB_TOKEN`.

## How the Risk Score Works

The score is a composite of three components (0-100):

| Component | Weight | What it measures |
|-----------|--------|------------------|
| Blast radius | 40% | How many downstream files could be affected by the changed functions |
| Change complexity | 30% | Total lines added + deleted across the PR |
| Guard violations | 30% | Severity of detected dangerous patterns (blocks = 15pts, warnings = 3pts) |

**Thresholds:**
- 0-24: Low (green)
- 25-49: Medium (yellow)
- 50-74: High (orange)
- 75-100: Critical (red) — fails the check by default

## Updating the Comment

The action uses an HTML comment marker (`<!-- savants-pr-analysis -->`) to identify its own comments. When a PR is updated (new commits pushed), it updates the existing comment instead of creating a new one.

## Troubleshooting

**Savants install fails**: The installer downloads from `releases.savants.dev` (Cloudflare R2) with GitHub releases as fallback. Check if your runner has internet access.

**No functions detected**: Savants uses tree-sitter for parsing. Supported languages: TypeScript, JavaScript, Python, Rust, Go, Java, Ruby, C, C++. If your language isn't supported, the blast radius section will be empty but guard checks still run.

**Risk score seems wrong**: The score is conservative by design. A large refactor touching many files will score high even if it's safe. Adjust `SAVANTS_RISK_THRESHOLD` or set it to `0` to use the check as informational only.
