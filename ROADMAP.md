# Savants Roadmap

## Priority 0: 60-second install-to-value (THE critical path)

Everything else is blocked by this. No GTM, no launch, no growth without frictionless install → immediate aha moment.

### P0.1 — Embedded graph (kill the FalkorDB port-forward dependency)
- [ ] Evaluate kuzu vs bundled FalkorDB subprocess vs DuckDB+graph extension
- [ ] Implement chosen embedded graph with migration shim over existing Cypher queries
- [ ] `savants init` creates `~/.savants/data/` and starts the graph automatically
- [ ] Verify all 24 MCP tools work against embedded graph
- [ ] Remove the `FALKORDB_HOST`/`FALKORDB_PORT` requirement for local mode

### P0.2 — Single binary distribution
- [ ] Package with PyInstaller or Nuitka (no visible Python dependency)
- [ ] `curl -fsSL savants.sh | sh` install script that detects OS/arch and downloads binary
- [ ] Test on: Ubuntu 22/24, Debian 12, Fedora 40, NixOS, macOS arm64/x86
- [ ] Binary size target: < 50MB
- [ ] Homebrew tap: `brew install savants`

### P0.3 — Auto-detect infrastructure
- [ ] `savants watch --auto-detect`: find kubeconfig contexts, Docker socket, systemd
- [ ] `savants scan .`: index the current repo's code graph automatically
- [ ] First useful output < 60 seconds from install
- [ ] Auto-detect output: "Found: 1 K8s cluster (astra-k3s, 94 pods), 1 host (astra, 8 cores), Docker active, 534 systemd units"

### P0.4 — The "aha moment" output
- [ ] After auto-detect, immediately show: top 3 issues found (failed units, CrashLoop pods, disk warnings, high-error pods)
- [ ] Color-coded severity, one-line-per-issue summary
- [ ] "Run `savants story` for full diagnosis" call-to-action
- [ ] `savants story` = cluster-wide + host-wide narrative (combines pod_story + host_story)

### P0.5 — savants.sh install script
- [ ] Write the install script (hosted at savants.sh)
- [ ] Detect OS, arch, download correct binary
- [ ] Add to PATH, verify install, run `savants --version`
- [ ] Deploy to savants.sh domain

---

## Priority 1: Complete the product (needed for launch)

### P1.1 — Host agent MCP tools
- [ ] `host_state` MCP tool (CPU, memory, disk, load, failed units)
- [ ] `host_story` MCP tool (significant journal/dmesg events, same pattern as pod_story)
- [ ] Wire host agent into CLI: `savants host watch` daemon
- [ ] Wire host ingestor into `savants watch --auto-detect`

### P1.2 — CAUSED_BY temporal correlation (Phase 7 from log intelligence checklist)
- [ ] Rolling window (60s) of recent cluster state changes from K8sWatcher
- [ ] When LogEvent emitted, check window for temporally-adjacent events
- [ ] Create CAUSED_BY edge with `confidence: "candidate"`
- [ ] Surface in pod_story: "likely caused by configmap edit 30s prior"

### P1.3 — `savants report` shareable output
- [ ] Generate markdown report of current cluster + host state + top issues
- [ ] One-command: `savants report > diagnosis.md`
- [ ] Designed to be pasted into Slack, GitHub issues, blog posts
- [ ] Include graph stats, severity histogram, top 10 events, mentioned entities

### P1.4 — Secret scrubber (prerequisite for cloud tier)
- [ ] Scrub ConfigKey values, string_refs, EnvVar from graph before any cloud sync
- [ ] Test: no actual secret values in any graph node property
- [ ] Configurable scrub rules (regex patterns, env var names)

---

## Priority 2: Launch (HN + distribution)

### P2.1 — Hacker News launch
- [ ] Write the HN post: "Savants: I built a tool that found the root cause of 15 crashing pods in 45 seconds"
- [ ] Include: real demo (astra-k3s diagnosis), install command, what it found
- [ ] Prepare for traffic: savants.sh serving binaries, savants.dev with docs
- [ ] Target: front page, 5k installs first week

### P2.2 — savants.dev landing page
- [ ] Hero: "Your infrastructure savant. Know what's wrong in 60 seconds."
- [ ] Demo video: `curl savants.sh | sh` → auto-detect → diagnosis
- [ ] Install command front and center
- [ ] Feature grid: code graph, K8s state, log intelligence, host monitoring
- [ ] "How it works" section: the 3-tier pipeline diagram

### P2.3 — MCP marketplace listings
- [ ] Claude Code MCP directory listing
- [ ] Cursor MCP marketplace
- [ ] README with MCP setup instructions
- [ ] `savants mcp install` command that configures .mcp.json automatically

### P2.4 — Documentation
- [ ] Getting started guide (install → first diagnosis in 5 minutes)
- [ ] CLI reference (all commands + flags)
- [ ] MCP tools reference (all 24+ tools with examples)
- [ ] Architecture overview (the 6-layer graph)

---

## Priority 3: Growth + monetization

### P3.1 — savants.cloud (the paid tier)
- [ ] Federation server: accept graph metadata from local clients
- [ ] Multi-tenant graph storage
- [ ] Cross-cluster queries ("which cluster has this configmap?")
- [ ] `savants connect` command to link local → cloud
- [ ] Team tier: $49/user/month
- [ ] SOC2 Type 2 (when cloud tier has customers)

### P3.2 — AWS Marketplace listing
- [ ] Package as AMI or container for Marketplace
- [ ] Metered billing integration
- [ ] Enterprise tier: SSO, RBAC, audit logs

### P3.3 — Extension API
- [ ] `@savants_tool` decorator for third-party MCP tools
- [ ] Plugin discovery and loading
- [ ] Example extensions: PagerDuty, Slack, Terraform drift
- [ ] Extension marketplace on savants.dev

### P3.4 — AWS/GCP/Azure cloud ingestors
- [ ] AWS: EC2, RDS, Lambda, ECS, CloudWatch metrics → graph
- [ ] GCP: GKE, Cloud Run, Cloud SQL → graph
- [ ] Azure: AKS, App Service → graph
- [ ] Cross-cloud federation queries

---

## Priority 4: Scale

### P4.1 — Performance (Rust hot paths)
- [ ] Move log classifier to Rust (target: 5M lines/sec/core)
- [ ] Move drain3 template extraction to Rust
- [ ] Move graph writer to batch Cypher with pipelining
- [ ] Profile and optimize for 50-cluster, 10k-pod scale

### P4.2 — macOS + Windows host agents
- [ ] macOS collector: sysctl, vm_stat, launchctl, `log show`
- [ ] Windows collector: WMI, Event Log, Windows Services
- [ ] Platform detection + automatic collector selection

### P4.3 — Web UI
- [ ] Graph visualization (nodes + edges, interactive)
- [ ] Incident timeline view
- [ ] Team dashboard for savants.cloud
- [ ] Compliance artifact export (SOC2, HIPAA incident evidence)

---

## What's already done (as of 2026-04-10)

- [x] Code graph: AST parsing, call graphs, decorators, config keys, episodic memory
- [x] K8s cluster state: diff-based ingest, real-time watch streams (1.27s propagation)
- [x] K8s log intelligence: 3-tier pipeline (classifier → drain3 → graph), MENTIONS edges, retention GC
- [x] Host agent: CPU/mem/disk/net/processes/systemd/docker/dmesg/journald (0.9s ingest)
- [x] MCP server: 24 tools including pod_story, cluster_state, diff_impact
- [x] CLI: `savants k8s watch`, `savants k8s snapshot`
- [x] Rename: synapcode → savants (complete across Python, Rust, desktop, tests, docs, CI)
- [x] Domains: savants.dev, savants.sh, savants.cloud purchased
- [x] Proven on live cluster: diagnosed coredns root cause (15 CrashLoopBackOff pods, 7.5M log lines → 1 story → 45 seconds)
