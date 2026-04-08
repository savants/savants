"""Tests for the secret scrubber. Pure unit tests, no FalkorDB needed."""

from __future__ import annotations

from synapcode.security.secret_scrub import scrub, is_secret_value, REDACTED


class TestSpecificPatterns:
    def test_aws_access_key(self):
        cleaned, hit = scrub("AKIAIOSFODNN7EXAMPLE")
        assert hit
        assert REDACTED in cleaned

    def test_openai_key(self):
        cleaned, hit = scrub("sk-proj-abc123def456ghi789jkl012mno345")
        assert hit
        assert REDACTED in cleaned

    def test_anthropic_key(self):
        cleaned, hit = scrub("sk-ant-api03-AbCdEf123456GhIjKlMnOpQr")
        assert hit
        assert REDACTED in cleaned

    def test_github_token(self):
        cleaned, hit = scrub("ghp_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789")
        assert hit
        assert REDACTED in cleaned

    def test_jwt(self):
        jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
        cleaned, hit = scrub(jwt)
        assert hit
        assert REDACTED in cleaned

    def test_postgres_uri_keeps_user_redacts_password(self):
        cleaned, hit = scrub("postgres://admin:hunter2@db.internal/prod")
        assert hit
        assert "admin" in cleaned, "username should be preserved"
        assert "hunter2" not in cleaned
        assert REDACTED in cleaned
        assert "db.internal/prod" in cleaned, "host should be preserved"

    def test_mongodb_uri(self):
        cleaned, hit = scrub("mongodb+srv://user:realpassword@cluster.mongodb.net/db")
        assert hit
        assert "realpassword" not in cleaned
        assert REDACTED in cleaned

    def test_password_assignment(self):
        cleaned, hit = scrub("password: hunter2")
        assert hit
        assert "hunter2" not in cleaned

    def test_password_assignment_with_quotes(self):
        cleaned, hit = scrub('database.password = "my-real-secret"')
        assert hit
        assert "my-real-secret" not in cleaned

    def test_pem_private_key(self):
        cleaned, hit = scrub("-----BEGIN RSA PRIVATE KEY-----\nMIIE...")
        assert hit
        assert REDACTED in cleaned


class TestHighEntropyFallback:
    def test_long_random_base64(self):
        # 40-char high entropy string — should be flagged
        secret = "aB3xK9pQ2mW7nR4tY8vL5jH6gF0sD1cVbN3xK9pQ"
        assert is_secret_value(secret)

    def test_long_random_hex(self):
        secret = "a3f5b8c1d2e4f6079083a4b5c6d7e8f901a2b3c4d5e6f70819203a4b5c6d7e8f"
        assert is_secret_value(secret)

    def test_short_lowercase_word_not_secret(self):
        assert not is_secret_value("hello")

    def test_long_lowercase_english_not_secret(self):
        # Low entropy even though long
        assert not is_secret_value("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")


class TestNonSecrets:
    def test_normal_function_name_not_secret(self):
        assert not is_secret_value("HandleTsCoinTransfer")

    def test_dotted_path_not_secret(self):
        assert not is_secret_value("synapcode.graph.cpg.CodePropertyGraphBuilder")

    def test_empty_string(self):
        cleaned, hit = scrub("")
        assert not hit
        assert cleaned == ""

    def test_short_normal_value(self):
        cleaned, hit = scrub("localhost")
        assert not hit
        assert cleaned == "localhost"

    def test_normal_url_no_creds(self):
        cleaned, hit = scrub("https://example.com/api/v1")
        assert not hit
        assert cleaned == "https://example.com/api/v1"

    def test_postgres_uri_without_password_not_redacted(self):
        # Note: this URI shape doesn't have user:pass@, so it shouldn't trigger
        cleaned, hit = scrub("postgres://localhost:5432/mydb")
        assert not hit
        assert cleaned == "postgres://localhost:5432/mydb"


class TestIntegrationWithParser:
    """Smoke tests proving the scrubber is wired into _flatten_config."""

    def test_config_value_with_secret_redacted(self, tmp_path):
        from synapcode.graph.cpg import _flatten_config

        node = {"database": {"password": "hunter2", "host": "db.local"}}
        out: list = []
        _flatten_config(node, "", "config.yaml", "yaml", out, depth=0)
        by_name = {k.name: k for k in out}
        assert "database.password" in by_name
        assert "hunter2" not in by_name["database.password"].value
        assert REDACTED in by_name["database.password"].value
        # Non-secret value untouched
        assert by_name["database.host"].value == "db.local"

    def test_string_literal_secret_rejected(self):
        from synapcode.graph.cpg import _looks_like_symbol

        # Normal symbol passes
        assert _looks_like_symbol("HandleTsCoinTransfer")
        # Secret-shaped value rejected even though it might pass shape
        assert not _looks_like_symbol("AKIAIOSFODNN7EXAMPLE")
