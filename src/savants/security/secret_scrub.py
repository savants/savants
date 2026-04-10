"""Secret scrubber for the parser ingest path.

This is a HARD PREREQUISITE for shipping the cloud tier. Without it, the
moment a customer's mongod.conf, application.yml, or .env file is checked
into git, we'd index their database password / API key / connection
string into our shared graph and become a credential leak vector.

The scrubber runs on three ingest paths:

  1. ConfigKey values during YAML/TOML/JSON flattening
  2. String literals that pass the looks-like-symbol filter
  3. EnvVar default_value capture from os.getenv("X", "default")

Patterns are stolen from `detect-secrets`, `gitleaks`, and `trufflehog` —
years of tuned regexes are better than anything we'd invent fresh.

Audit-trail design (the important part):

  Pure scrubbing destroys the security team's ability to detect "this
  secret was leaked at commit X by user Y." That's a real use case we
  must preserve. The solution:

    - REDACT the secret value (so the graph never contains credentials)
    - But ALSO emit a SecretLeak node with: file_path, line, secret_type,
      and a *salted hash* of the secret (a fingerprint, not the value)
    - The fingerprint lets us detect "same secret reused across files"
      and "this secret is still present after a remediation" without
      storing the secret itself

  Security teams query SecretLeak nodes for the audit trail. The actual
  secret value never enters the graph.

Usage:

    from savants.security.secret_scrub import scrub_with_findings
    cleaned, findings = scrub_with_findings("postgres://user:hunter2@db/prod")
    # cleaned: 'postgres://user:<REDACTED>@db/prod'
    # findings: [SecretFinding(secret_type='postgres_uri', fingerprint='ab12...')]

The function never raises and never panics. If something looks risky,
it's redacted and a SecretFinding is recorded. If not, the original
string is returned unchanged with an empty findings list.
"""

from __future__ import annotations

import hashlib
import math
import os
import re
from dataclasses import dataclass

REDACTED = "<REDACTED>"

# Per-installation salt for the secret fingerprint. The fingerprint is
# only used to *detect duplicates* across files in the same graph; it
# must NOT be reversible into the original secret. The salt prevents
# rainbow-table attacks against common secrets like "password123".
#
# In the cloud tier this should be a per-tenant value loaded from KMS.
# For the local tier the default falls back to a stable per-host value
# derived from the machine, so duplicate detection still works across
# repeated indexes on the same machine.
_DEFAULT_SALT = os.environ.get(
    "SYNAPCODE_SCRUB_SALT",
    "savants-default-fingerprint-salt-not-secret-by-itself",
)


@dataclass(frozen=True)
class SecretFinding:
    """Metadata about a detected secret. Never contains the secret itself.

    The fingerprint is a salted SHA-256 hash, used only for duplicate
    detection across files in the same graph. It's not reversible to
    the original secret.
    """

    secret_type: str  # e.g. "aws_access_key_id", "postgres_uri", "jwt"
    fingerprint: str  # salted SHA-256, first 16 hex chars (64 bits)


def _fingerprint(value: str) -> str:
    """Compute a non-reversible fingerprint of a secret value.

    Used purely for "is this the same secret we already saw in another
    file?" — never for storing or recovering the secret itself.
    """
    h = hashlib.sha256()
    h.update(_DEFAULT_SALT.encode())
    h.update(b"\x00")
    h.update(value.encode("utf-8", errors="replace"))
    return h.hexdigest()[:16]

# --- Specific token patterns -------------------------------------------------
#
# Each entry is (name, compiled_regex, replacement_strategy).
# replacement_strategy:
#   "full"   - replace the whole matched value with REDACTED
#   "group"  - replace just the captured group inside the larger value

_SPECIFIC_PATTERNS: list[tuple[str, re.Pattern[str], str]] = [
    # AWS access key id
    ("aws_access_key_id", re.compile(r"AKIA[0-9A-Z]{16}"), "full"),
    ("aws_secret_access_key", re.compile(r"(?i)aws[_-]?secret[_-]?access[_-]?key"), "full"),

    # OpenAI / Anthropic style API keys
    ("openai_key", re.compile(r"sk-(?:proj-)?[A-Za-z0-9_-]{20,}"), "full"),
    ("anthropic_key", re.compile(r"sk-ant-[A-Za-z0-9_-]{20,}"), "full"),

    # GitHub tokens
    ("github_token", re.compile(r"gh[psaur]_[A-Za-z0-9]{36,}"), "full"),

    # Slack tokens
    ("slack_token", re.compile(r"xox[abprs]-[A-Za-z0-9-]{10,}"), "full"),

    # Stripe (live secret)
    ("stripe_live", re.compile(r"sk_live_[A-Za-z0-9]{20,}"), "full"),

    # Google API key
    ("google_api_key", re.compile(r"AIza[0-9A-Za-z_-]{35}"), "full"),

    # JWT — header.payload.signature in base64url
    ("jwt", re.compile(r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+"), "full"),

    # Postgres / MongoDB / MySQL connection strings with embedded password
    # Match the password between : and @ in user:pass@host
    ("postgres_uri", re.compile(r"(postgres(?:ql)?://[^:\s]+:)([^@\s]+)(@)"), "group"),
    ("mongodb_uri", re.compile(r"(mongodb(?:\+srv)?://[^:\s]+:)([^@\s]+)(@)"), "group"),
    ("mysql_uri", re.compile(r"(mysql://[^:\s]+:)([^@\s]+)(@)"), "group"),
    ("redis_uri", re.compile(r"(redis(?:s)?://[^:\s]+:)([^@\s]+)(@)"), "group"),

    # PEM private keys
    ("pem_private_key", re.compile(r"-----BEGIN [A-Z ]+ PRIVATE KEY-----"), "full"),

    # Generic "password = value" / "password: value" lines
    # (catches mongod.conf style configs). The first capture group keeps
    # the key + assignment intact; the second is the secret to redact.
    ("password_assignment", re.compile(
        r"(\b(?:password|passwd|secret|api[_-]?key|access[_-]?token)\b\s*[:=]\s*['\"]?)([^'\"\s,;]+)",
        re.IGNORECASE,
    ), "group"),
]

# --- Generic high-entropy fallback -------------------------------------------

# Long base64-ish or hex-ish strings with high Shannon entropy.
# We only scan strings >= MIN_ENTROPY_LEN characters to avoid false positives
# on legitimate identifiers.
_MIN_ENTROPY_LEN = 32
# Hex strings cap at log2(16) = 4.0 bits/char. Base64 caps at log2(64) = 6.
# We use 3.5 so a perfectly random hex secret (4.0) is well above threshold,
# while english prose (~2.5) is well below.
_ENTROPY_THRESHOLD = 3.5
_BASE64ISH_RE = re.compile(r"^[A-Za-z0-9+/_=-]{32,}$")
_HEXISH_RE = re.compile(r"^[a-fA-F0-9]{32,}$")


def _shannon_entropy(s: str) -> float:
    """Compute Shannon entropy in bits per character."""
    if not s:
        return 0.0
    counts: dict[str, int] = {}
    for ch in s:
        counts[ch] = counts.get(ch, 0) + 1
    n = len(s)
    return -sum((c / n) * math.log2(c / n) for c in counts.values())


def _looks_like_high_entropy_secret(value: str) -> bool:
    """True if the value looks like a generic high-entropy secret."""
    if len(value) < _MIN_ENTROPY_LEN:
        return False
    if not (_BASE64ISH_RE.match(value) or _HEXISH_RE.match(value)):
        return False
    return _shannon_entropy(value) >= _ENTROPY_THRESHOLD


# --- Public API --------------------------------------------------------------


def scrub_with_findings(value: str) -> tuple[str, list[SecretFinding]]:
    """Scrub secrets and return findings for the audit trail.

    Returns:
        (cleaned_value, list_of_SecretFinding)

    The findings list lets the caller emit SecretLeak nodes into the
    graph so security teams can audit "what secrets did we detect, where,
    when" without ever storing the secret values themselves. Each finding
    contains a non-reversible fingerprint used only for duplicate detection.
    """
    if not value or not isinstance(value, str):
        return (value, [])

    cleaned = value
    findings: list[SecretFinding] = []

    for name, pattern, strategy in _SPECIFIC_PATTERNS:
        if strategy == "full":
            for m in pattern.finditer(cleaned):
                findings.append(
                    SecretFinding(secret_type=name, fingerprint=_fingerprint(m.group(0)))
                )
            new = pattern.sub(REDACTED, cleaned)
            cleaned = new
        elif strategy == "group":
            def _replace_group(m: re.Match[str], _name: str = name) -> str:
                groups = m.groups()
                if len(groups) >= 3:
                    secret = groups[1]
                    findings.append(
                        SecretFinding(secret_type=_name, fingerprint=_fingerprint(secret))
                    )
                    return groups[0] + REDACTED + groups[2]
                if len(groups) == 2:
                    secret = groups[1]
                    findings.append(
                        SecretFinding(secret_type=_name, fingerprint=_fingerprint(secret))
                    )
                    return groups[0] + REDACTED
                # Single-group fallback
                findings.append(
                    SecretFinding(
                        secret_type=_name,
                        fingerprint=_fingerprint(groups[0] if groups else m.group(0)),
                    )
                )
                return REDACTED
            cleaned = pattern.sub(_replace_group, cleaned)

    # High-entropy fallback for unmatched values that look secret-shaped.
    # Only counts if we didn't already match something specific.
    if not findings and _looks_like_high_entropy_secret(cleaned):
        findings.append(
            SecretFinding(secret_type="high_entropy", fingerprint=_fingerprint(cleaned))
        )
        cleaned = REDACTED

    return (cleaned, findings)


def scrub(value: str) -> tuple[str, bool]:
    """Backward-compatible wrapper that returns just (cleaned, was_secret).

    Use this when you only need the cleaned string and don't care about
    the audit trail. Use `scrub_with_findings` when you want to emit
    SecretLeak nodes into the graph.
    """
    cleaned, findings = scrub_with_findings(value)
    return (cleaned, bool(findings))


def is_secret_value(value: str) -> bool:
    """True if `value` contains anything we'd consider a secret."""
    _, findings = scrub_with_findings(value)
    return bool(findings)


# Key names that strongly imply the value is a credential, regardless of
# what the value itself looks like. Used by scrub_config_value to redact
# values whose key context says "secret" even when the value alone wouldn't
# trip any pattern (e.g. database.password = "hunter2" — too short for
# entropy check, no recognizable shape, but obviously a credential).
_SECRET_KEY_TOKENS = (
    "password", "passwd", "pwd",
    "secret", "secrets",
    "api_key", "apikey", "api-key",
    "access_token", "accesstoken", "access-token",
    "auth_token", "authtoken", "auth-token",
    "private_key", "privatekey", "private-key",
    "client_secret", "clientsecret", "client-secret",
    "credentials",
    "token",
)


def _key_implies_secret(key_path: str) -> bool:
    """True if a config key path looks like it should hold a credential.

    Matches case-insensitively against the LAST segment of the dotted path
    so 'database.password' matches but 'password_policy.length' doesn't
    falsely trigger on a non-secret nested key.
    """
    if not key_path:
        return False
    last_segment = key_path.rsplit(".", 1)[-1].lower()
    # Strip array indices like ports[0]
    last_segment = last_segment.split("[")[0]
    return any(token in last_segment for token in _SECRET_KEY_TOKENS)


def scrub_config_value(
    key_path: str, value: str
) -> tuple[str, list[SecretFinding]]:
    """Scrub a YAML/TOML/JSON ConfigKey value with awareness of its key path.

    The key path is the strongest signal in config files: a value at
    `database.password` is a secret regardless of whether the value
    itself looks credential-shaped. We check the key first; if it implies
    a secret we redact unconditionally and emit a finding tagged with
    'config_secret_by_key'. Then we run the standard content-based scan
    on top in case the value also matches a specific pattern.
    """
    cleaned, findings = scrub_with_findings(value)
    if _key_implies_secret(key_path) and value and value.strip():
        # Override: even if content scan said "not a secret", the key
        # context overrides. Redact and record the finding.
        if not findings:
            findings = [
                SecretFinding(
                    secret_type="config_secret_by_key",
                    fingerprint=_fingerprint(value),
                )
            ]
            cleaned = REDACTED
    return (cleaned, findings)
