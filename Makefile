# Savants Makefile
#
# On your Mac:
#   git clone https://git.bernad.in/miguel/savants.git
#   cd savants
#   make setup
#   make run
#
# That's it.

ASTRA_IP ?= 100.95.164.99
PORT ?= 6379
BIN = savants-cli/target/release/savants

.PHONY: setup build run status story mcp install clean

# First time setup: install Rust + build + configure
setup: build install mcp
	@echo ""
	@echo "✅ Savants ready. Run: make run"

# Build the binary
build:
	@echo "Building Savants..."
	@cd savants-cli && cargo build --release
	@echo "✅ Built: $(BIN)"

# Install to /usr/local/bin
install: build
	@cp $(BIN) /usr/local/bin/savants 2>/dev/null || sudo cp $(BIN) /usr/local/bin/savants
	@echo "✅ Installed to /usr/local/bin/savants"

# Configure MCP for Claude Code / Cursor
mcp:
	@SAVANTS_PORT=$(PORT) SAVANTS_HOST=$(ASTRA_IP) savants mcp install
	@echo "✅ MCP configured. Restart your AI tool."

# Quick status check
run:
	@SAVANTS_PORT=$(PORT) SAVANTS_HOST=$(ASTRA_IP) savants status

status:
	@SAVANTS_PORT=$(PORT) SAVANTS_HOST=$(ASTRA_IP) savants status

story:
	@SAVANTS_PORT=$(PORT) SAVANTS_HOST=$(ASTRA_IP) savants story

diagnose:
	@SAVANTS_PORT=$(PORT) SAVANTS_HOST=$(ASTRA_IP) savants story --since-minutes 60

costs:
	@SAVANTS_PORT=$(PORT) SAVANTS_HOST=$(ASTRA_IP) savants serve <<< '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"make","version":"1.0"}}}\n{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"advanced_graph_query","arguments":{"query":"MATCH (c:CloudCost) WHERE c.cost_30d > 10 RETURN c.provider, c.service, c.cost_30d ORDER BY c.cost_30d DESC"}}}' 2>/dev/null | tail -1 | python3 -c "import sys,json;d=json.loads(sys.stdin.read());print(d.get('result',{}).get('content',[{}])[0].get('text',''))" 2>/dev/null

# Start daemon (on astra only, not laptop)
daemon-start:
	@SAVANTS_PORT=$(PORT) SAVANTS_GOTIFY_URL=http://10.43.16.5:80 SAVANTS_GOTIFY_TOKEN=AcUC9NcjcMGLXtm savants daemon start

daemon-stop:
	@SAVANTS_PORT=$(PORT) savants daemon stop

daemon-status:
	@SAVANTS_PORT=$(PORT) savants daemon status

daemon-logs:
	@SAVANTS_PORT=$(PORT) savants daemon logs

clean:
	@cd savants-cli && cargo clean
	@echo "Cleaned build artifacts"
