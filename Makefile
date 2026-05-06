# Savants Makefile
#
# Build:     make              (standard build, /proc fallback)
# eBPF:      make ebpf         (with kernel tracing, needs nix-shell for libelf)
# Deploy:    make deploy       (install + restart agent)
# Agent:     make agent-start  (start as root with eBPF)
#            make agent-stop
#            make agent-status
#
# First time: make setup

ASTRA_IP ?= 100.95.164.99
BIN = savants-cli/target/release/savants
INSTALL_DIR = $(HOME)/.savants/bin
CLANG = $(shell which clang 2>/dev/null || find /nix/store -maxdepth 3 -name "clang" -type f 2>/dev/null | head -1)
TOKEN = $(shell python3 -c "import json;print(json.load(open('$(HOME)/.savants/state.json')).get('cloud_token',''))" 2>/dev/null)

.PHONY: build ebpf bpf-compile deploy agent-start agent-stop agent-status agent-logs setup install mcp clean

# Default: build without eBPF (works everywhere)
build:
	@echo "Building Savants..."
	@cd savants-cli && cargo build --release
	@echo "Built: $(BIN) ($$(du -h $(BIN) | cut -f1))"

# Full build: compile BPF program + build with libbpf-rs
ebpf: bpf-compile
	@echo "Building Savants with eBPF support..."
	@cd savants-cli && nix-shell --extra-experimental-features flakes shell.nix \
		--run "cargo build --release --features ebpf"
	@echo "Built with eBPF: $(BIN) ($$(du -h $(BIN) | cut -f1))"

# Compile the BPF C program to bytecode (embedded in binary at compile time)
bpf-compile:
	@echo "Compiling BPF program..."
	@$(CLANG) -O2 -g -target bpf -D__TARGET_ARCH_x86 -fno-addrsig \
		-c savants-cli/ebpf/tcp_retransmit.bpf.c \
		-I savants-cli/ebpf \
		-o savants-cli/ebpf/tcp_retransmit.bpf.o
	@echo "BPF: savants-cli/ebpf/tcp_retransmit.bpf.o ($$(du -h savants-cli/ebpf/tcp_retransmit.bpf.o | cut -f1))"

# Deploy: install binary + set capabilities + restart agent
deploy:
	@cp $(BIN) $(INSTALL_DIR)/savants.new && mv $(INSTALL_DIR)/savants.new $(INSTALL_DIR)/savants
	@sudo setcap cap_bpf,cap_perfmon,cap_net_admin+ep $(INSTALL_DIR)/savants 2>/dev/null || true
	@echo "Deployed to $(INSTALL_DIR)/savants"

# Build + deploy in one step
ship: ebpf deploy agent-restart
	@echo "Shipped."

# Start agent as root (required for eBPF)
agent-start:
	@echo "Starting agent..."
	@sudo bash -c "export SAVANTS_TOKEN='$(TOKEN)' && \
		/home/miguel/.savants/bin/savants agent run > /tmp/savants-agent.log 2>&1 &"
	@sleep 3
	@grep -E "eBPF|Polling" /tmp/savants-agent.log 2>/dev/null || true
	@echo "Agent started. Logs: /tmp/savants-agent.log"

agent-stop:
	@sudo pkill -9 -f "savants agent" 2>/dev/null || true
	@echo "Agent stopped."

agent-restart: agent-stop
	@sleep 2
	@$(MAKE) agent-start

agent-status:
	@PID=$$(pgrep -f "savants agent" | head -1); \
	if [ -n "$$PID" ]; then \
		echo "Agent running (PID $$PID)"; \
		grep -E "eBPF|watch" /tmp/savants-agent.log 2>/dev/null | tail -5; \
	else \
		echo "Agent not running."; \
	fi

agent-logs:
	@tail -30 /tmp/savants-agent.log 2>/dev/null || echo "No logs."

# First time setup
setup: ebpf deploy
	@echo ""
	@echo "Savants ready. Run: make agent-start"

install: build
	@cp $(BIN) $(INSTALL_DIR)/savants.new && mv $(INSTALL_DIR)/savants.new $(INSTALL_DIR)/savants
	@echo "Installed to $(INSTALL_DIR)/savants"

mcp:
	@savants mcp install 2>/dev/null || echo "Run 'savants mcp install' manually"

clean:
	@cd savants-cli && cargo clean
	@rm -f savants-cli/ebpf/tcp_retransmit.bpf.o
	@echo "Cleaned."
