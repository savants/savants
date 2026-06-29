# Contributing to Savants

## Quick start

```bash
# Clone
git clone https://github.com/savants/savants.git
cd savants

# Run Python SDK tests
cd packages/guard-python
pip install -e ".[dev]"
pytest tests/ -v

# Run Rust CLI check
cd savants-cli
cargo check
```

## Development workflow

1. Create a branch from `main`
2. Make changes
3. Run tests: `make test-python` and `make test-rust`
4. Open a pull request

## Testing

```bash
make test-python      # 148 Python tests (unit + e2e + integrations + risk)
make test-rust        # Rust compilation check
make test-container   # Container test (installs from PyPI)
make test-e2e         # Full pipeline (local + container)
make test-all         # Everything in parallel
```

## Python SDK

The Python SDK is at `packages/guard-python/`. It has zero required dependencies.

### Adding a new action type

1. Add the action to `BLOCKING_ACTIONS` or `SOFT_ACTIONS` in `guard.py`
2. Update the parser regex in `parser.py` if needed
3. Add tests to `tests/test_guard.py`
4. Update the README.md action types table

### Adding a framework integration

1. Add the integration function to `integrations.py`
2. Use lazy imports (import the framework inside the function, not at module level)
3. Raise `ImportError` with install instructions if the framework isn't available
4. Add mock-based tests to `tests/test_integrations.py`

### Adding a guard profile

1. Create a JSON file in `packages/guard-profiles/presets/`
2. Each rule is a DSL string: `"when <field> <op> '<value>' then <action>"`
3. Add the profile to the `profiles` command in `scripts/guard-cli.sh`

## Rust CLI

The CLI is at `savants-cli/`. It uses Clap for argument parsing and serde for JSON.

### Guard hook changes

The guard hook intercept is in `savants-cli/src/commands/hooks.rs`. Exit codes:
- `0` = allow the tool call
- `2` = block the tool call

## Commit messages

Use conventional commits:

```
feat: add new guard action type
fix: correct DSL parser for quoted values
docs: update getting-started guide
test: add e2e tests for secret detection
```

## Code style

- Python: follow existing patterns, no external formatters required
- Rust: `cargo fmt` before committing
- No trailing whitespace, UTF-8 encoding, LF line endings

## Releases

Releases are managed by the maintainers. The release pipeline:

```bash
make release-guard    # publish SDK + deploy docs + deploy cloud
make test-agent       # no-context agent validation (10 tasks)
make sync-github      # push public subset to GitHub
```

## License

By contributing, you agree that your contributions will be licensed under the project's FSL-1.1-Apache-2.0 license (CLI) or MIT license (Python SDK).
