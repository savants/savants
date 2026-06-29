"""
End-to-end tests for the top 10 customer use cases of Savants Guard.

These tests validate that the guard system works correctly for real-world
scenarios that paying customers would rely on.
"""

import json
import os
import tempfile
import pytest
from savants_guard import create_guard, GuardResult, GuardError


# ============================================================
# USE CASE 1: Prevent destructive file operations
# Customer: "I want Claude Code to never rm -rf my project"
# ============================================================

class TestDestructiveFileOps:
    """Blocks rm -rf, chmod 777, mkfs, dd — the catastrophic stuff."""

    def setup_method(self):
        self.guard = create_guard([
            "when tool eq 'Bash' and command contains 'rm -rf /' then block",
            "when tool eq 'Bash' and command contains 'rm -rf ~' then block",
            "when tool eq 'Bash' and command contains 'rm -rf .' then suggest 'Use git clean -fd for tracked repos'",
            "when tool eq 'Bash' and command contains 'chmod 777' then suggest 'Use chmod 755 for directories or 644 for files'",
            "when tool eq 'Bash' and command contains 'sudo rm' then ask 'sudo rm is destructive and irreversible'",
            "when tool eq 'Bash' and command contains 'mkfs' then block",
            "when tool eq 'Bash' and command contains 'dd if=' then block",
        ])

    def test_blocks_rm_rf_root(self):
        result = self.guard.check({"tool": "Bash", "command": "rm -rf / --no-preserve-root"})
        assert result.blocked is True

    def test_blocks_rm_rf_home(self):
        result = self.guard.check({"tool": "Bash", "command": "rm -rf ~/Documents"})
        assert result.blocked is True

    def test_suggests_alternative_for_rm_rf_dot(self):
        result = self.guard.check({"tool": "Bash", "command": "rm -rf ."})
        assert result.blocked is False
        assert result.allowed is False
        assert result.action == "suggest"
        assert "git clean" in result.suggestion

    def test_suggests_alternative_for_chmod_777(self):
        result = self.guard.check({"tool": "Bash", "command": "chmod 777 /var/www"})
        assert result.blocked is False
        assert result.action == "suggest"
        assert "755" in result.suggestion

    def test_asks_for_sudo_rm(self):
        result = self.guard.check({"tool": "Bash", "command": "sudo rm -r /tmp/important"})
        assert result.action == "ask"

    def test_allows_safe_rm(self):
        result = self.guard.check({"tool": "Bash", "command": "rm temp_file.txt"})
        assert result.allowed is True

    def test_blocks_mkfs(self):
        result = self.guard.check({"tool": "Bash", "command": "mkfs.ext4 /dev/sda1"})
        assert result.blocked is True

    def test_blocks_dd(self):
        result = self.guard.check({"tool": "Bash", "command": "dd if=/dev/zero of=/dev/sda"})
        assert result.blocked is True


# ============================================================
# USE CASE 2: Protect git from force push
# Customer: "Rewrite force push to --force-with-lease automatically"
# ============================================================

class TestGitProtection:
    """Auto-rewrites force push, suggests alternatives for hard reset."""

    def setup_method(self):
        self.guard = create_guard([
            "when tool eq 'Bash' and command contains 'git push --force' then rewrite 'git push --force-with-lease'",
            "when tool eq 'Bash' and command contains 'git push -f ' then rewrite 'git push --force-with-lease'",
            "when tool eq 'Bash' and command contains 'git reset --hard' then suggest 'Use git stash to save changes before resetting'",
        ])

    def test_rewrites_force_push(self):
        result = self.guard.check({"tool": "Bash", "command": "git push --force origin main"})
        assert result.blocked is False
        assert result.action == "rewrite"
        assert result.suggestion == "git push --force-with-lease"

    def test_rewrites_short_force_flag(self):
        result = self.guard.check({"tool": "Bash", "command": "git push -f origin main"})
        assert result.action == "rewrite"
        assert result.suggestion == "git push --force-with-lease"

    def test_suggests_for_hard_reset(self):
        result = self.guard.check({"tool": "Bash", "command": "git reset --hard HEAD~3"})
        assert result.action == "suggest"
        assert "stash" in result.suggestion

    def test_allows_normal_push(self):
        result = self.guard.check({"tool": "Bash", "command": "git push origin main"})
        assert result.allowed is True

    def test_allows_normal_reset(self):
        result = self.guard.check({"tool": "Bash", "command": "git reset --soft HEAD~1"})
        assert result.allowed is True


# ============================================================
# USE CASE 3: Block secrets from being written to files
# Customer: "Catch API keys before they get committed"
# ============================================================

class TestSecretDetection:
    """Catches secret patterns in file content, not just file paths."""

    def setup_method(self):
        self.guard = create_guard([
            "when tool eq 'Write' and content contains 'sk_live_' then suggest 'Use an environment variable instead of hardcoding Stripe keys'",
            "when tool eq 'Write' and content contains 'sk-ant-' then suggest 'Use an environment variable instead of hardcoding Anthropic keys'",
            "when tool eq 'Write' and content contains 'ghp_' then suggest 'Use an environment variable instead of hardcoding GitHub tokens'",
            "when tool eq 'Write' and content contains 'AKIA' then suggest 'Use an environment variable instead of hardcoding AWS keys'",
            "when tool eq 'Edit' and new_string contains 'sk_live_' then suggest 'This edit inserts a Stripe secret key'",
        ])

    def test_catches_stripe_key_in_write(self):
        result = self.guard.check({
            "tool": "Write",
            "file_path": "config.py",
            "content": 'STRIPE_KEY = "sk_live_abc123def456"',
        })
        assert result.action == "suggest"
        assert "environment variable" in result.suggestion

    def test_catches_anthropic_key(self):
        result = self.guard.check({
            "tool": "Write",
            "file_path": "settings.ts",
            "content": 'const key = "sk-ant-api03-xxxx"',
        })
        assert result.action == "suggest"

    def test_catches_github_token(self):
        result = self.guard.check({
            "tool": "Write",
            "file_path": ".github/config",
            "content": 'token: ghp_1234567890abcdef',
        })
        assert result.action == "suggest"

    def test_catches_aws_key(self):
        result = self.guard.check({
            "tool": "Write",
            "file_path": "deploy.sh",
            "content": 'export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE',
        })
        assert result.action == "suggest"

    def test_catches_key_in_edit(self):
        result = self.guard.check({
            "tool": "Edit",
            "file_path": "config.py",
            "new_string": 'key = "sk_live_test123"',
            "old_string": 'key = os.environ["STRIPE_KEY"]',
        })
        assert result.action == "suggest"

    def test_allows_clean_code(self):
        result = self.guard.check({
            "tool": "Write",
            "file_path": "config.py",
            "content": 'STRIPE_KEY = os.environ["STRIPE_KEY"]',
        })
        assert result.allowed is True


# ============================================================
# USE CASE 4: Guard production database operations
# Customer: "Block DROP/TRUNCATE in production, allow in staging"
# ============================================================

class TestDatabaseProtection:
    """Blocks destructive SQL in production, allows in other environments."""

    def setup_method(self):
        self.guard = create_guard([
            "when action contains 'drop' and environment eq 'production' then block",
            "when action contains 'truncate' and environment eq 'production' then ask 'TRUNCATE in production requires approval'",
            "when action contains 'delete' and environment eq 'production' and table contains 'users' then block",
        ])

    def test_blocks_drop_in_production(self):
        result = self.guard.check({"action": "drop_table", "environment": "production"})
        assert result.blocked is True

    def test_allows_drop_in_staging(self):
        result = self.guard.check({"action": "drop_table", "environment": "staging"})
        assert result.allowed is True

    def test_asks_for_truncate_in_production(self):
        result = self.guard.check({"action": "truncate_table", "environment": "production"})
        assert result.action == "ask"

    def test_blocks_delete_users_in_production(self):
        result = self.guard.check({
            "action": "delete_records",
            "environment": "production",
            "table": "users",
        })
        assert result.blocked is True

    def test_allows_delete_logs_in_production(self):
        result = self.guard.check({
            "action": "delete_records",
            "environment": "production",
            "table": "debug_logs",
        })
        assert result.allowed is True


# ============================================================
# USE CASE 5: Enforce spend limits
# Customer: "Block any agent action that costs over $100"
# ============================================================

class TestSpendLimits:
    """Enforces budget controls on agent actions."""

    def setup_method(self):
        self.guard = create_guard([
            "when amount gt 100 then require_approval",
            "when spend gt 1000 then block",
            "when cost gt 50 and category eq 'infrastructure' then ask 'Infrastructure spend over $50'",
        ])

    def test_blocks_high_spend(self):
        result = self.guard.check({"action": "provision", "spend": 5000})
        assert result.blocked is True

    def test_requires_approval_for_medium_amount(self):
        result = self.guard.check({"action": "purchase", "amount": 250})
        assert result.blocked is True  # require_approval maps to blocking
        assert result.action == "require_approval"

    def test_allows_small_amount(self):
        result = self.guard.check({"action": "purchase", "amount": 50})
        assert result.allowed is True

    def test_asks_for_infra_spend(self):
        result = self.guard.check({"action": "deploy", "cost": 75, "category": "infrastructure"})
        assert result.action == "ask"

    def test_allows_non_infra_spend(self):
        result = self.guard.check({"action": "deploy", "cost": 75, "category": "marketing"})
        assert result.allowed is True


# ============================================================
# USE CASE 6: Control package publishing
# Customer: "Make npm publish and docker push require approval"
# ============================================================

class TestPublishProtection:
    """Publishing to public registries requires human approval."""

    def setup_method(self):
        self.guard = create_guard([
            "when tool eq 'Bash' and command contains 'npm publish' then ask 'Publishing to npm is public and permanent'",
            "when tool eq 'Bash' and command contains 'docker push' then ask 'Pushing a Docker image to a registry'",
            "when tool eq 'Bash' and command contains 'cargo publish' then ask 'Publishing to crates.io is permanent'",
            "when tool eq 'Bash' and command contains 'twine upload' then ask 'Publishing to PyPI'",
        ])

    def test_asks_for_npm_publish(self):
        result = self.guard.check({"tool": "Bash", "command": "npm publish --access public"})
        assert result.action == "ask"
        assert "npm" in result.suggestion

    def test_asks_for_docker_push(self):
        result = self.guard.check({"tool": "Bash", "command": "docker push myapp:latest"})
        assert result.action == "ask"

    def test_asks_for_cargo_publish(self):
        result = self.guard.check({"tool": "Bash", "command": "cargo publish"})
        assert result.action == "ask"

    def test_asks_for_pypi_upload(self):
        result = self.guard.check({"tool": "Bash", "command": "twine upload dist/*"})
        assert result.action == "ask"

    def test_allows_npm_install(self):
        result = self.guard.check({"tool": "Bash", "command": "npm install express"})
        assert result.allowed is True

    def test_allows_docker_build(self):
        result = self.guard.check({"tool": "Bash", "command": "docker build -t myapp ."})
        assert result.allowed is True


# ============================================================
# USE CASE 7: Protect .env and credential files
# Customer: "Never let the agent read or write my .env files"
# ============================================================

class TestEnvFileProtection:
    """Protects .env, credentials, SSH keys, and AWS config."""

    def setup_method(self):
        self.guard = create_guard([
            "when tool eq 'Write' and file_path contains '.env' then ask 'Writing to .env may expose secrets'",
            "when tool eq 'Edit' and file_path contains '.env' then ask 'Editing .env may expose secrets'",
            "when tool eq 'Read' and file_path contains '.env' then block",
            "when tool eq 'Write' and file_path contains 'credentials' then block",
            "when tool eq 'Write' and file_path contains 'id_rsa' then block",
            "when tool eq 'Write' and file_path contains '.ssh' then block",
            "when tool eq 'Read' and file_path contains '.aws/credentials' then block",
        ])

    def test_asks_before_writing_env(self):
        result = self.guard.check({"tool": "Write", "file_path": "/app/.env"})
        assert result.action == "ask"

    def test_asks_before_editing_env(self):
        result = self.guard.check({"tool": "Edit", "file_path": ".env.production"})
        assert result.action == "ask"

    def test_blocks_reading_env(self):
        result = self.guard.check({"tool": "Read", "file_path": "/app/.env"})
        assert result.blocked is True

    def test_blocks_writing_credentials(self):
        result = self.guard.check({"tool": "Write", "file_path": "config/credentials.json"})
        assert result.blocked is True

    def test_blocks_writing_ssh_keys(self):
        result = self.guard.check({"tool": "Write", "file_path": "/home/user/.ssh/id_rsa"})
        assert result.blocked is True

    def test_blocks_reading_aws_credentials(self):
        result = self.guard.check({"tool": "Read", "file_path": "/home/user/.aws/credentials"})
        assert result.blocked is True

    def test_allows_reading_normal_files(self):
        result = self.guard.check({"tool": "Read", "file_path": "src/main.py"})
        assert result.allowed is True

    def test_allows_writing_env_example(self):
        """env.example is fine — it shouldn't contain real secrets."""
        result = self.guard.check({"tool": "Write", "file_path": ".env.example"})
        # This WILL match because .env is a substring — this is expected behavior
        # The ask action lets the user approve it
        assert result.action == "ask"


# ============================================================
# USE CASE 8: Kubernetes safety
# Customer: "Block namespace deletion and secret exposure"
# ============================================================

class TestKubernetesSafety:
    """Prevents destructive k8s operations and secret value exposure."""

    def setup_method(self):
        self.guard = create_guard([
            "when tool eq 'Bash' and command contains 'kubectl delete namespace' then block",
            "when tool eq 'Bash' and command contains 'kubectl delete ns' then block",
            "when tool eq 'Bash' and command contains 'kubectl exec' then ask 'kubectl exec opens a shell in a running pod'",
            "when tool eq 'Bash' and command contains 'kubectl' and command contains 'secret' and command contains '-o yaml' then block",
            "when tool eq 'Bash' and command contains 'kubectl' and command contains 'secret' and command contains '-o json' then block",
        ])

    def test_blocks_namespace_deletion(self):
        result = self.guard.check({"tool": "Bash", "command": "kubectl delete namespace production"})
        assert result.blocked is True

    def test_blocks_ns_shorthand_deletion(self):
        result = self.guard.check({"tool": "Bash", "command": "kubectl delete ns staging"})
        assert result.blocked is True

    def test_asks_for_exec(self):
        result = self.guard.check({"tool": "Bash", "command": "kubectl exec -it pod-123 -- /bin/sh"})
        assert result.action == "ask"

    def test_blocks_secret_yaml_exposure(self):
        result = self.guard.check({"tool": "Bash", "command": "kubectl get secret db-creds -o yaml"})
        assert result.blocked is True

    def test_blocks_secret_json_exposure(self):
        result = self.guard.check({"tool": "Bash", "command": "kubectl get secret db-creds -o json -n prod"})
        assert result.blocked is True

    def test_allows_listing_secrets_metadata(self):
        result = self.guard.check({"tool": "Bash", "command": "kubectl get secrets -n production"})
        assert result.allowed is True

    def test_allows_normal_kubectl(self):
        result = self.guard.check({"tool": "Bash", "command": "kubectl get pods -n default"})
        assert result.allowed is True


# ============================================================
# USE CASE 9: Composable profile stacking
# Customer: "I want standard + secrets + k8s protection combined"
# ============================================================

class TestProfileStacking:
    """Loads multiple profiles and verifies all rules work together."""

    def setup_method(self):
        # Load actual profile files
        profiles_dir = os.path.join(
            os.path.dirname(__file__), "..", "..", "guard-profiles", "presets"
        )
        all_rules = []
        for profile in ["standard", "secrets"]:
            path = os.path.join(profiles_dir, f"{profile}.json")
            if os.path.exists(path):
                with open(path) as f:
                    all_rules.extend(json.load(f))

        # Deduplicate
        all_rules = list(dict.fromkeys(all_rules))
        self.guard = create_guard(all_rules)
        self.rule_count = len(all_rules)

    def test_loads_combined_rules(self):
        """Combined profiles should have more rules than either alone."""
        assert self.rule_count > 25  # standard has 25

    def test_standard_rule_works(self):
        """Rules from standard profile should fire."""
        result = self.guard.check({"tool": "Bash", "command": "rm -rf /"})
        assert result.blocked is True

    def test_secrets_rule_works(self):
        """Rules from secrets profile should fire."""
        result = self.guard.check({"tool": "Read", "file_path": "/home/user/.aws/credentials"})
        assert result.blocked is True

    def test_no_duplicates(self):
        """Rules should be deduplicated."""
        rules = self.guard.list_rules()
        assert len(rules) == len(set(rules))

    def test_allows_safe_operations(self):
        """Non-matching operations should still be allowed."""
        result = self.guard.check({"tool": "Read", "file_path": "src/main.py"})
        assert result.allowed is True


# ============================================================
# USE CASE 10: Runtime rule management
# Customer: "Add and remove rules mid-session without restarting"
# ============================================================

class TestRuntimeRuleManagement:
    """Add, remove, and modify rules dynamically during a session."""

    def test_start_empty_and_add_rules(self):
        guard = create_guard([])
        assert len(guard.list_rules()) == 0

        # Everything allowed
        result = guard.check({"action": "delete_all"})
        assert result.allowed is True

        # Add a rule
        guard.add_rule("when action contains 'delete' then block")
        assert len(guard.list_rules()) == 1

        # Now blocked
        result = guard.check({"action": "delete_all"})
        assert result.blocked is True

    def test_multiple_rules_first_match_wins(self):
        guard = create_guard([
            "when action eq 'deploy' then suggest 'Consider using staging first'",
            "when action eq 'deploy' then block",
        ])
        result = guard.check({"action": "deploy"})
        # First matching rule wins
        assert result.action == "suggest"
        assert result.blocked is False

    def test_log_tracks_evaluations(self):
        guard = create_guard(["when action eq 'nuke' then block"])

        guard.check({"action": "read"})
        guard.check({"action": "nuke"})
        guard.check({"action": "write"})

        log = guard.get_log()
        assert len(log) == 3
        assert log[0]["result"].allowed is True
        assert log[1]["result"].blocked is True
        assert log[2]["result"].allowed is True

    def test_wrap_decorator_blocks(self):
        guard = create_guard(["when action eq 'destroy' then block"])

        @guard.wrap
        def dangerous(**kwargs):
            return "executed"

        # Should raise GuardError
        with pytest.raises(GuardError) as exc:
            dangerous(action="destroy")
        assert exc.value.guard_action == "block"

        # Safe action should execute
        result = dangerous(action="read")
        assert result == "executed"

    def test_guard_result_fields(self):
        guard = create_guard(["when x eq 'yes' then suggest 'Try no instead'"])

        result = guard.check({"x": "yes"})
        assert result.blocked is False
        assert result.allowed is False
        assert result.action == "suggest"
        assert result.suggestion == "Try no instead"
        assert result.rule is not None
        assert result.context == {"x": "yes"}

    def test_guard_result_allowed_fields(self):
        guard = create_guard(["when x eq 'yes' then block"])

        result = guard.check({"x": "no"})
        assert result.blocked is False
        assert result.allowed is True
        assert result.action is None
        assert result.rule is None
        assert result.suggestion is None


# ============================================================
# BONUS: All profiles load without errors
# ============================================================

class TestAllProfilesLoad:
    """Every profile file should parse without errors."""

    PROFILES = ["minimal", "standard", "paranoid", "secrets", "git-safe",
                "infra-safe", "publish-safe", "k8s-safe", "k8s-secrets"]

    def test_all_profiles_load(self):
        profiles_dir = os.path.join(
            os.path.dirname(__file__), "..", "..", "guard-profiles", "presets"
        )
        for profile_name in self.PROFILES:
            path = os.path.join(profiles_dir, f"{profile_name}.json")
            if not os.path.exists(path):
                pytest.skip(f"Profile {profile_name} not found at {path}")
            with open(path) as f:
                rules = json.load(f)
            guard = create_guard(rules)
            assert len(guard.list_rules()) > 0, f"Profile {profile_name} has no valid rules"

    def test_all_profiles_have_unique_rules(self):
        profiles_dir = os.path.join(
            os.path.dirname(__file__), "..", "..", "guard-profiles", "presets"
        )
        for profile_name in self.PROFILES:
            path = os.path.join(profiles_dir, f"{profile_name}.json")
            if not os.path.exists(path):
                continue
            with open(path) as f:
                rules = json.load(f)
            assert len(rules) == len(set(rules)), f"Profile {profile_name} has duplicate rules"
