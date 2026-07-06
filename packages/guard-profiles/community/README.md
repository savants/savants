# Community Guard Profiles

User-contributed guard profiles for specific tools, languages, and workflows.

## How to use a community profile

```bash
# Download and install a community profile
savants guard install nixos-safe

# Or manually: copy the JSON to your custom profiles
mkdir -p ~/.savants/custom-profiles
curl -fsSL https://raw.githubusercontent.com/savants/savants/main/packages/guard-profiles/community/nixos-safe.json \
  -o ~/.savants/custom-profiles/nixos-safe.json
savants guard preset standard+nixos-safe
```

## How to contribute a profile

1. Create a JSON file with your guard rules
2. Name it descriptively: `{tool}-safe.json` or `{workflow}-safe.json`
3. Open a PR to add it to `packages/guard-profiles/community/`

### Rule format

```json
[
  "when tool eq 'Bash' and command contains 'dangerous-thing' then block",
  "when tool eq 'Bash' and command contains 'risky-thing' then ask 'Why this needs approval'",
  "when tool eq 'Bash' and command contains 'fixable-thing' then suggest 'Safer alternative'",
  "when tool eq 'Bash' and command contains 'replaceable' then rewrite 'safer-command'"
]
```

### Actions

| Action | Behavior |
|--------|----------|
| `block` | Hard stop — command prevented |
| `suggest 'msg'` | Denied with alternative — agent auto-recovers |
| `rewrite 'cmd'` | Silently replaces command |
| `ask 'reason'` | Prompts user for approval |

## Available community profiles

| Profile | Rules | Description |
|---------|-------|-------------|
| [nixos-safe](nixos-safe.json) | 6 | NixOS: enforce --flake, prevent nix-env, protect /etc/nixos |
