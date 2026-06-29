# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Savants, please report it responsibly.

**Do NOT open a public GitHub issue for security vulnerabilities.**

Email: **hello@savants.dev**

Include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

We will acknowledge receipt within 48 hours and provide a timeline for resolution.

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.4.x   | Yes       |
| 0.3.x   | Yes       |
| < 0.3   | No        |

## Security Design

Savants Guard is designed with security as a core principle:

- **Local-first**: Guard rules evaluate in-process with zero network calls. No data leaves your machine unless you opt into managed mode.
- **Deterministic**: Same input always produces the same output. No LLM in the evaluation path.
- **No secret storage**: Savants never stores or logs secret values. MCP call arguments are hashed before logging.
- **Minimal permissions**: The CLI runs as your user. No sudo, no root, no kernel access required for guard features.
- **Credential redaction**: `savants mcp audit` redacts credential values when displaying MCP server configurations.
