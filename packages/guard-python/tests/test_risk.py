"""Tests for graph-powered risk scoring (savants_guard.risk)."""

import pytest
from savants_guard import (
    create_guard,
    Guard,
    RiskContext,
    file_risk,
    change_size,
    risk_aware_safety,
)
from savants_guard.risk import _extract_var_names


# ============================================================
# Built-in: file_risk
# ============================================================

class TestFileRisk:
    def test_high_risk_payment(self):
        assert file_risk({"file": "src/payments/charge.py"}) == "high"

    def test_high_risk_auth(self):
        assert file_risk({"file": "lib/auth/login.ts"}) == "high"

    def test_high_risk_billing(self):
        assert file_risk({"file": "services/billing/invoice.py"}) == "high"

    def test_high_risk_migration(self):
        assert file_risk({"file": "db/migrations/001_add_users.sql"}) == "high"

    def test_high_risk_secret(self):
        assert file_risk({"file": "config/secrets.yaml"}) == "high"

    def test_high_risk_credential(self):
        assert file_risk({"file_path": "src/credential_store.py"}) == "high"

    def test_high_risk_password(self):
        assert file_risk({"path": "utils/password_hash.py"}) == "high"

    def test_high_risk_token(self):
        assert file_risk({"file": "src/token_manager.py"}) == "high"

    def test_low_risk_normal_file(self):
        assert file_risk({"file": "src/components/Button.tsx"}) == "low"

    def test_low_risk_readme(self):
        assert file_risk({"file": "README.md"}) == "low"

    def test_no_file_in_context(self):
        assert file_risk({"action": "edit"}) == "low"

    def test_case_insensitive(self):
        assert file_risk({"file": "src/Payment/Handler.java"}) == "high"


# ============================================================
# Built-in: change_size
# ============================================================

class TestChangeSize:
    def test_counts_lines(self):
        diff = "line1\nline2\nline3\n"
        assert change_size({"diff": diff}) == 3

    def test_empty_diff(self):
        assert change_size({"diff": ""}) == 0

    def test_no_diff_key(self):
        assert change_size({"action": "edit"}) == 0

    def test_change_key(self):
        assert change_size({"change": "a\nb\nc"}) == 3

    def test_single_line(self):
        assert change_size({"diff": "one line"}) == 1


# ============================================================
# Variable extraction from AST
# ============================================================

class TestExtractVarNames:
    def test_simple_var(self):
        node = {"type": "var", "name": "blast_radius"}
        assert _extract_var_names(node) == {"blast_radius"}

    def test_compare_node(self):
        node = {
            "type": "compare",
            "op": "gt",
            "left": {"type": "var", "name": "caller_count"},
            "right": {"type": "literal", "value": 20},
        }
        assert _extract_var_names(node) == {"caller_count"}

    def test_and_node(self):
        node = {
            "type": "and",
            "children": [
                {"type": "compare", "op": "eq", "left": {"type": "var", "name": "file_risk"}, "right": {"type": "literal", "value": "high"}},
                {"type": "compare", "op": "contains", "left": {"type": "var", "name": "action"}, "right": {"type": "literal", "value": "edit"}},
            ],
        }
        assert _extract_var_names(node) == {"file_risk", "action"}

    def test_none_node(self):
        assert _extract_var_names(None) == set()


# ============================================================
# RiskContext enrichment
# ============================================================

class TestRiskContext:
    def test_default_sources(self):
        rc = RiskContext()
        assert "file_risk" in rc.source_names
        assert "change_size" in rc.source_names

    def test_register_custom_source(self):
        rc = RiskContext()
        rc.register_risk_source("blast_radius", lambda ctx: 42)
        assert "blast_radius" in rc.source_names

    def test_unregister_source(self):
        rc = RiskContext()
        rc.register_risk_source("custom", lambda ctx: 1)
        rc.unregister_risk_source("custom")
        assert "custom" not in rc.source_names

    def test_enrich_adds_risk_variable(self):
        rc = RiskContext()
        rc.register_risk_source("blast_radius", lambda ctx: 15)
        condition = {
            "type": "compare",
            "op": "gt",
            "left": {"type": "var", "name": "blast_radius"},
            "right": {"type": "literal", "value": 10},
        }
        enriched = rc.enrich({"action": "edit"}, condition)
        assert enriched["blast_radius"] == 15
        assert enriched["action"] == "edit"

    def test_enrich_does_not_mutate_original(self):
        rc = RiskContext()
        rc.register_risk_source("blast_radius", lambda ctx: 15)
        condition = {
            "type": "compare",
            "op": "gt",
            "left": {"type": "var", "name": "blast_radius"},
            "right": {"type": "literal", "value": 10},
        }
        original = {"action": "edit"}
        rc.enrich(original, condition)
        assert "blast_radius" not in original

    def test_enrich_does_not_overwrite_existing(self):
        rc = RiskContext()
        rc.register_risk_source("blast_radius", lambda ctx: 99)
        condition = {
            "type": "compare",
            "op": "gt",
            "left": {"type": "var", "name": "blast_radius"},
            "right": {"type": "literal", "value": 10},
        }
        enriched = rc.enrich({"blast_radius": 5}, condition)
        assert enriched["blast_radius"] == 5  # original preserved

    def test_enrich_returns_original_when_no_sources_needed(self):
        rc = RiskContext()
        condition = {
            "type": "compare",
            "op": "eq",
            "left": {"type": "var", "name": "action"},
            "right": {"type": "literal", "value": "delete"},
        }
        ctx = {"action": "delete"}
        result = rc.enrich(ctx, condition)
        assert result is ctx  # same object, no copy


# ============================================================
# Lazy evaluation — source only called when referenced
# ============================================================

class TestLazyEvaluation:
    def test_source_not_called_when_not_referenced(self):
        call_count = {"n": 0}

        def expensive_source(ctx):
            call_count["n"] += 1
            return 50

        rc = RiskContext()
        rc.register_risk_source("blast_radius", expensive_source)

        # Rule only references 'action', not 'blast_radius'
        guard = create_guard(
            ["when action contains 'delete' then block"],
            risk_context=rc,
        )
        guard.check({"action": "delete_user"})
        assert call_count["n"] == 0

    def test_source_called_when_referenced(self):
        call_count = {"n": 0}

        def blast_source(ctx):
            call_count["n"] += 1
            return 50

        rc = RiskContext()
        rc.register_risk_source("blast_radius", blast_source)

        guard = create_guard(
            ["when blast_radius gt 30 then require_approval"],
            risk_context=rc,
        )
        guard.check({"file": "src/main.py"})
        assert call_count["n"] == 1

    def test_source_called_once_per_check(self):
        """Even if multiple rules reference same var, source called per rule."""
        call_count = {"n": 0}

        def blast_source(ctx):
            call_count["n"] += 1
            return 10  # below threshold, so both rules are evaluated

        rc = RiskContext()
        rc.register_risk_source("blast_radius", blast_source)

        guard = create_guard(
            [
                "when blast_radius gt 30 then block",
                "when blast_radius gt 50 then require_approval",
            ],
            risk_context=rc,
        )
        guard.check({"file": "test.py"})
        # Both rules evaluated (neither blocks), so source called twice
        assert call_count["n"] == 2


# ============================================================
# Guard integration with risk_context
# ============================================================

class TestGuardWithRisk:
    def test_file_risk_blocks_high_risk_edit(self):
        rc = RiskContext()
        guard = create_guard(
            ["when file_risk eq 'high' and action contains 'edit' then ask 'This file is high-risk'"],
            risk_context=rc,
        )
        result = guard.check({"file": "src/auth/session.py", "action": "edit_file"})
        assert result.blocked is False  # 'ask' is a soft action
        assert result.action == "ask"
        assert result.suggestion == "This file is high-risk"

    def test_file_risk_allows_low_risk(self):
        rc = RiskContext()
        guard = create_guard(
            ["when file_risk eq 'high' and action contains 'edit' then block"],
            risk_context=rc,
        )
        result = guard.check({"file": "src/utils/helpers.py", "action": "edit_file"})
        assert result.blocked is False
        assert result.allowed is True

    def test_change_size_triggers_on_large_diff(self):
        rc = RiskContext()
        guard = create_guard(
            ["when change_size gt 500 then ask 'Large change'"],
            risk_context=rc,
        )
        big_diff = "\n".join(f"line {i}" for i in range(600))
        result = guard.check({"diff": big_diff, "action": "commit"})
        assert result.action == "ask"

    def test_change_size_allows_small_diff(self):
        rc = RiskContext()
        guard = create_guard(
            ["when change_size gt 500 then ask 'Large change'"],
            risk_context=rc,
        )
        result = guard.check({"diff": "one\ntwo\nthree", "action": "commit"})
        assert result.allowed is True

    def test_custom_blast_radius_blocks(self):
        rc = RiskContext()
        rc.register_risk_source("blast_radius", lambda ctx: 45)

        guard = create_guard(
            ["when blast_radius gt 30 then require_approval"],
            risk_context=rc,
        )
        result = guard.check({"file": "anything.py"})
        assert result.blocked is True
        assert result.action == "require_approval"

    def test_custom_caller_count_suggests(self):
        rc = RiskContext()
        rc.register_risk_source("caller_count", lambda ctx: 25)

        guard = create_guard(
            ["when caller_count gt 20 then suggest 'Many callers'"],
            risk_context=rc,
        )
        result = guard.check({"function": "handle_request"})
        assert result.action == "suggest"
        assert result.suggestion == "Many callers"

    def test_no_risk_context_works_normally(self):
        """Guard without risk_context still works fine."""
        guard = create_guard(["when action eq 'delete' then block"])
        result = guard.check({"action": "delete"})
        assert result.blocked is True


# ============================================================
# risk_aware_safety preset
# ============================================================

class TestRiskAwareSafetyPreset:
    def test_preset_creates_guard(self):
        guard = risk_aware_safety()
        assert isinstance(guard, Guard)
        rules = guard.list_rules()
        assert len(rules) == 4

    def test_preset_with_custom_context(self):
        rc = RiskContext()
        rc.register_risk_source("blast_radius", lambda ctx: 50)
        rc.register_risk_source("caller_count", lambda ctx: 5)

        guard = risk_aware_safety(rc)
        result = guard.check({"file": "src/main.py", "action": "edit"})
        # blast_radius 50 > 30 -> require_approval (first rule wins)
        assert result.blocked is True
        assert result.action == "require_approval"

    def test_preset_file_risk_rule(self):
        guard = risk_aware_safety()
        result = guard.check({"file": "src/payment/stripe.py", "action": "edit"})
        # file_risk = 'high' and action contains 'edit' -> ask
        assert result.action == "ask"
        assert "high-risk" in (result.suggestion or "")

    def test_preset_change_size_rule(self):
        guard = risk_aware_safety()
        big_diff = "\n".join(f"+ added line {i}" for i in range(600))
        result = guard.check({"diff": big_diff, "action": "commit"})
        # change_size 600 > 500 -> ask
        assert result.action == "ask"

    def test_preset_allows_safe_action(self):
        guard = risk_aware_safety()
        result = guard.check({"file": "src/utils/format.py", "action": "read", "diff": "one line"})
        assert result.allowed is True
