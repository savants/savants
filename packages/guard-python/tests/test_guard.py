"""Tests for savants-guard Python SDK."""

import pytest
from savants_guard import (
    create_guard,
    GuardError,
    GuardResult,
    parse_rule,
    production_safety,
    spend_limit,
    business_hours,
    deploy_safety,
)


class TestCreateGuard:
    def test_basic_block(self):
        guard = create_guard(["when action eq 'delete' then block"])
        result = guard.check({"action": "delete"})
        assert result.blocked is True
        assert result.action == "block"
        assert result.rule == "when action eq 'delete' then block"

    def test_basic_allow(self):
        guard = create_guard(["when action eq 'delete' then block"])
        result = guard.check({"action": "read"})
        assert result.blocked is False
        assert result.allowed is True
        assert result.rule is None

    def test_contains_operator(self):
        guard = create_guard(["when action contains 'delete' then block"])
        assert guard.check({"action": "delete_database"}).blocked is True
        assert guard.check({"action": "DELETE_USER"}).blocked is True  # case insensitive
        assert guard.check({"action": "read_logs"}).blocked is False

    def test_gt_operator(self):
        guard = create_guard(["when spend gt 100 then block"])
        assert guard.check({"spend": 150}).blocked is True
        assert guard.check({"spend": 100}).blocked is False
        assert guard.check({"spend": 50}).blocked is False

    def test_and_combinator(self):
        guard = create_guard(["when action contains 'delete' and env eq 'production' then block"])
        assert guard.check({"action": "delete_db", "env": "production"}).blocked is True
        assert guard.check({"action": "delete_db", "env": "staging"}).blocked is False
        assert guard.check({"action": "read_logs", "env": "production"}).blocked is False

    def test_or_combinator(self):
        guard = create_guard(["when env eq 'staging' or env eq 'development' then allow"])
        # 'allow' is not a blocking action, so blocked should be False
        result = guard.check({"env": "staging"})
        assert result.blocked is False

    def test_multiple_rules(self):
        guard = create_guard([
            "when action contains 'delete' then block",
            "when spend gt 100 then require_approval",
        ])
        r1 = guard.check({"action": "delete_db"})
        assert r1.blocked is True
        assert r1.action == "block"

        r2 = guard.check({"action": "purchase", "spend": 200})
        assert r2.blocked is True
        assert r2.action == "require_approval"

    def test_require_approval(self):
        guard = create_guard(["when amount gt 500 then require_approval"])
        result = guard.check({"action": "transfer", "amount": 1000})
        assert result.blocked is True
        assert result.action == "require_approval"

    def test_invalid_rules_ignored(self):
        guard = create_guard([
            "not a valid rule",
            "# comment",
            "// another comment",
            "when action eq 'delete' then block",
        ])
        assert len(guard.list_rules()) == 1

    def test_add_rule(self):
        guard = create_guard([])
        assert len(guard.list_rules()) == 0
        guard.add_rule("when x eq 1 then block")
        assert len(guard.list_rules()) == 1
        assert guard.check({"x": 1}).blocked is True

    def test_list_rules(self):
        guard = create_guard([
            "when a eq 1 then block",
            "when b eq 2 then block",
        ])
        rules = guard.list_rules()
        assert len(rules) == 2
        assert "when a eq 1 then block" in rules

    def test_get_log(self):
        guard = create_guard(["when x eq 1 then block"])
        guard.check({"x": 1})
        guard.check({"x": 2})
        log = guard.get_log()
        assert len(log) == 2
        assert log[0]["result"].blocked is True
        assert log[1]["result"].blocked is False


class TestWrap:
    def test_wrap_blocks(self):
        guard = create_guard(["when action eq 'delete' then block"])

        @guard.wrap
        def dangerous_action(**kwargs):
            return "executed"

        with pytest.raises(GuardError) as exc_info:
            dangerous_action(action="delete")

        assert exc_info.value.guard_action == "block"

    def test_wrap_allows(self):
        guard = create_guard(["when action eq 'delete' then block"])

        @guard.wrap
        def safe_action(**kwargs):
            return "executed"

        result = safe_action(action="read")
        assert result == "executed"


class TestPresets:
    def test_production_safety(self):
        guard = production_safety()
        assert guard.check({"action": "delete_user", "environment": "production"}).blocked is True
        assert guard.check({"action": "delete_user", "environment": "staging"}).blocked is False
        assert guard.check({"action": "read_user", "environment": "production"}).blocked is False

    def test_spend_limit(self):
        guard = spend_limit(100)
        assert guard.check({"amount": 150}).blocked is True
        assert guard.check({"amount": 50}).blocked is False
        assert guard.check({"spend": 200}).blocked is True

    def test_business_hours(self):
        guard = business_hours()
        assert guard.check({"day_of_week": "Saturday"}).blocked is True
        assert guard.check({"day_of_week": "Monday"}).blocked is False

    def test_deploy_safety(self):
        guard = deploy_safety()
        assert guard.check({"action": "deploy", "risk_score": 0.9, "day_of_week": "Friday"}).blocked is True
        assert guard.check({"action": "deploy", "test_pass_rate": 95}).blocked is True
        assert guard.check({"skip_final_snapshot": True}).blocked is True


class TestParseRule:
    def test_simple_rule(self):
        result = parse_rule("when action eq 'delete' then block")
        assert result is not None
        assert result.action == "block"
        assert result.dsl == "when action eq 'delete' then block"

    def test_if_syntax(self):
        result = parse_rule("if action eq 'delete' then block")
        assert result is not None

    def test_invalid_rule(self):
        assert parse_rule("not a valid rule") is None
        assert parse_rule("") is None
        assert parse_rule("# comment") is None
        assert parse_rule("// comment") is None

    def test_numeric_value(self):
        result = parse_rule("when spend gt 100 then block")
        assert result is not None
        assert result.condition["right"]["value"] == 100

    def test_quoted_string(self):
        result = parse_rule("when env eq 'production' then block")
        assert result is not None
        assert result.condition["right"]["value"] == "production"


class TestSoftActions:
    def test_suggest_not_hard_block(self):
        guard = create_guard(["when action eq 'rm' then suggest 'Use trash-put instead'"])
        result = guard.check({"action": "rm"})
        assert result.blocked is False  # Not a hard block
        assert result.allowed is False  # But not allowed either
        assert result.action == "suggest"
        assert result.suggestion == "Use trash-put instead"

    def test_rewrite_returns_replacement(self):
        guard = create_guard(["when command contains 'git push --force' then rewrite 'git push --force-with-lease'"])
        result = guard.check({"command": "git push --force origin main"})
        assert result.blocked is False
        assert result.allowed is False
        assert result.action == "rewrite"
        assert result.suggestion == "git push --force-with-lease"

    def test_ask_escalates(self):
        guard = create_guard(["when action eq 'deploy' then ask 'Deploy requires approval'"])
        result = guard.check({"action": "deploy"})
        assert result.blocked is False
        assert result.allowed is False
        assert result.action == "ask"
        assert result.suggestion == "Deploy requires approval"

    def test_suggest_no_match_is_allowed(self):
        guard = create_guard(["when action eq 'rm' then suggest 'Use trash'"])
        result = guard.check({"action": "ls"})
        assert result.blocked is False
        assert result.allowed is True
        assert result.suggestion is None

    def test_block_still_hard_blocks(self):
        """Ensure block rules still work as before."""
        guard = create_guard(["when action eq 'nuke' then block"])
        result = guard.check({"action": "nuke"})
        assert result.blocked is True
        assert result.allowed is False

    def test_mixed_rules(self):
        """Soft rules are checked in order — first match wins."""
        guard = create_guard([
            "when action eq 'delete' then suggest 'Consider archiving instead'",
            "when action eq 'delete' then block",
        ])
        result = guard.check({"action": "delete"})
        # First rule wins
        assert result.action == "suggest"
        assert result.blocked is False
        assert result.suggestion == "Consider archiving instead"


class TestGuardError:
    def test_guard_error_properties(self):
        err = GuardError("when x then block", "block", {"x": 1})
        assert err.rule == "when x then block"
        assert err.guard_action == "block"
        assert err.context == {"x": 1}
        assert "Guard blocked" in str(err)
