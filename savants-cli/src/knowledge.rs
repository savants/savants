//! Knowledge base: known issue patterns and suggested fixes.
//!
//! Each pattern has:
//! - A regex or keyword match against log templates / resource states
//! - A severity classification
//! - A human-readable explanation of what's happening
//! - A suggested fix (command, config change, or investigation step)
//!
//! This is the "brain" that turns raw data into actionable advice.
//! The knowledge base is compiled into the binary — it's part of the
//! proprietary value that makes Savants worth paying for.

/// A known issue pattern with diagnosis and fix suggestion.
pub struct KnownPattern {
    pub id: &'static str,
    pub category: Category,
    pub severity: Severity,
    /// Keywords that match against log templates or resource properties
    pub keywords: &'static [&'static str],
    /// What this issue means in plain English
    pub explanation: &'static str,
    /// Suggested fix — actionable, specific
    pub fix: &'static str,
    /// What to investigate if the fix doesn't work
    pub investigate: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Category {
    Dns,
    Network,
    Memory,
    Disk,
    Certificate,
    Database,
    Config,
    Permission,
    Crash,
    Performance,
    Security,
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub enum Severity {
    Info,
    Warning,
    Error,
    Critical,
}

/// The compiled knowledge base.
pub static PATTERNS: &[KnownPattern] = &[
    // ── DNS ──
    KnownPattern {
        id: "dns-upstream-timeout",
        category: Category::Dns,
        severity: Severity::Critical,
        keywords: &["i/o timeout", "read udp", "plugin/errors", "coredns"],
        explanation: "CoreDNS is timing out trying to reach its upstream DNS server. \
                      This causes cluster-wide DNS failures — every pod that needs to \
                      resolve a hostname will fail.",
        fix: "Check the coredns ConfigMap for the upstream DNS server:\n\
              kubectl -n kube-system get configmap coredns -o yaml\n\
              If it points to an unreliable server (e.g., 100.100.100.100 for Tailscale),\n\
              change it to a public resolver:\n\
              forward . 1.1.1.1 8.8.8.8",
        investigate: "If the upstream is correct, check network connectivity from the \
                     coredns pod to the upstream server. The node's network interface \
                     may be flapping.",
    },
    KnownPattern {
        id: "dns-resolution-failed",
        category: Category::Dns,
        severity: Severity::Error,
        keywords: &["lookup", "on 10.43.0.10:53", "read udp", "connection refused"],
        explanation: "A pod cannot resolve a DNS name via the cluster DNS service (10.43.0.10). \
                      This usually means coredns is down or overloaded.",
        fix: "Check coredns health:\n\
              kubectl -n kube-system get pods -l k8s-app=kube-dns\n\
              If in CrashLoopBackOff, check coredns logs and ConfigMap.",
        investigate: "Check if the DNS service IP (10.43.0.10) is correct:\n\
                     kubectl -n kube-system get svc kube-dns",
    },

    // ── Network ──
    KnownPattern {
        id: "connection-refused",
        category: Category::Network,
        severity: Severity::Error,
        keywords: &["connect: connection refused", "dial tcp"],
        explanation: "A pod is trying to connect to a service that isn't listening. \
                      The target pod may be down, restarting, or the service IP may be wrong.",
        fix: "Check if the target service is running:\n\
              kubectl get pods -A | grep <service-name>\n\
              If the target pod is CrashLoopBackOff, fix that first.",
        investigate: "Verify the service endpoint resolves correctly:\n\
                     kubectl get endpoints <service-name> -n <namespace>",
    },
    KnownPattern {
        id: "tls-handshake-timeout",
        category: Category::Network,
        severity: Severity::Error,
        keywords: &["TLS handshake timeout", "net/http"],
        explanation: "A pod cannot complete a TLS handshake with the API server or \
                      an external service. This often indicates network connectivity \
                      issues or cert-manager problems.",
        fix: "Check if cert-manager is healthy:\n\
              kubectl -n cert-manager get pods\n\
              If cert-manager is in CrashLoopBackOff, it's likely a cascading failure \
              from DNS issues — fix DNS first.",
        investigate: "Check if the node can reach the API server:\n\
                     curl -k https://10.43.0.1:443/healthz",
    },
    KnownPattern {
        id: "dhcp-dad-conflict",
        category: Category::Network,
        severity: Severity::Warning,
        keywords: &["DAD detected", "dhcpcd"],
        explanation: "DHCP Duplicate Address Detection found another device using the \
                      same IP address. The host's network will flap until resolved. \
                      This causes intermittent connectivity for all services.",
        fix: "Set a static IP for this host to avoid DHCP conflicts entirely.\n\
              Find the conflicting device's MAC address in the log and either:\n\
              1. Give this host a static IP outside the DHCP range\n\
              2. Create a DHCP reservation on the router",
        investigate: "Check which device has the conflicting MAC:\n\
                     ip neigh show | grep <conflicting-ip>",
    },

    // ── Memory ──
    KnownPattern {
        id: "oom-killed",
        category: Category::Memory,
        severity: Severity::Critical,
        keywords: &["OOMKilled", "oom-kill", "Out of memory", "invoked oom-killer"],
        explanation: "A process was killed by the kernel's Out-of-Memory killer. \
                      The container or host ran out of memory.",
        fix: "Increase the memory limit for the affected container:\n\
              kubectl edit deployment <name> -n <namespace>\n\
              Increase resources.limits.memory\n\
              Or investigate the memory leak in the application.",
        investigate: "Check current memory usage:\n\
                     kubectl top pods -n <namespace> --sort-by=memory",
    },

    // ── Disk ──
    KnownPattern {
        id: "disk-full",
        category: Category::Disk,
        severity: Severity::Critical,
        keywords: &["No space left on device", "ENOSPC", "disk full"],
        explanation: "A filesystem is full. Pods cannot write logs, databases cannot \
                      write data, and new containers cannot start.",
        fix: "Identify and clean up disk usage:\n\
              df -h  # find the full mount\n\
              du -sh /* | sort -rh | head -20  # find large directories\n\
              docker system prune  # clean Docker if applicable\n\
              crictl rmi --prune  # clean container images",
        investigate: "Check if log rotation is configured. Persistent Volume Claims \
                     may need resizing.",
    },

    // ── Config ──
    KnownPattern {
        id: "missing-api-key",
        category: Category::Config,
        severity: Severity::Error,
        keywords: &["No API key found", "API key", "auth", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
        explanation: "An application is missing a required API key. It cannot \
                      authenticate with an external service.",
        fix: "Create or mount the missing secret:\n\
              kubectl create secret generic <name> \\\n\
                --from-literal=API_KEY=<your-key> -n <namespace>\n\
              Then reference it in the deployment's env or envFrom.",
        investigate: "Check if the secret exists but isn't mounted:\n\
                     kubectl get secrets -n <namespace>",
    },
    KnownPattern {
        id: "eaddrinuse",
        category: Category::Config,
        severity: Severity::Error,
        keywords: &["EADDRINUSE", "address already in use"],
        explanation: "A process is trying to bind to a port that's already in use. \
                      Two instances of the same service may be running, or a previous \
                      instance didn't shut down cleanly.",
        fix: "Find what's using the port:\n\
              ss -tlnp | grep <port>\n\
              Kill the old process or change the port configuration.",
        investigate: "If in K8s, check for duplicate deployments or zombie pods:\n\
                     kubectl get pods -n <namespace> | grep <service-name>",
    },

    // ── Leader election ──
    KnownPattern {
        id: "leader-election-lost",
        category: Category::Network,
        severity: Severity::Error,
        keywords: &["leader election lost", "error retrieving resource lock", "lease"],
        explanation: "A controller lost its leader election lease. This usually means \
                      it can't reach the API server — often caused by DNS or network issues.",
        fix: "This is usually a symptom, not a root cause. Check:\n\
              1. Is coredns healthy? (DNS → API server reachability)\n\
              2. Is the node under memory/CPU pressure?\n\
              3. Are there network connectivity issues?\n\
              Fix the underlying issue and the leader election will recover automatically.",
        investigate: "Check the controller's logs for the specific lease name:\n\
                     kubectl logs <pod> -n <namespace> | grep lease",
    },

    // ── Database ──
    KnownPattern {
        id: "database-connection-failed",
        category: Category::Database,
        severity: Severity::Error,
        keywords: &["unable to refresh database connection pool", "no usable database connection",
                    "ECONNREFUSED", "connect ECONNREFUSED"],
        explanation: "An application cannot connect to its database. The database pod \
                      may be down, or DNS resolution for the database hostname is failing.",
        fix: "Check if the database pod is running:\n\
              kubectl get pods -A | grep postgres  # or mysql, redis, etc.\n\
              If the database is healthy, check if DNS can resolve its service name.",
        investigate: "Test connectivity from the application pod:\n\
                     kubectl exec <app-pod> -- nc -zv <db-host> <db-port>",
    },

    // ── Crash ──
    KnownPattern {
        id: "crash-loop-backoff",
        category: Category::Crash,
        severity: Severity::Critical,
        keywords: &["CrashLoopBackOff", "Back-off restarting failed container"],
        explanation: "A pod is repeatedly crashing and Kubernetes is backing off \
                      before restarting it. The container exits immediately after starting.",
        fix: "Check the pod logs for the crash reason:\n\
              kubectl logs <pod> -n <namespace> --previous\n\
              Common causes: missing config, wrong image tag, OOM, missing dependencies.",
        investigate: "Check pod events:\n\
                     kubectl describe pod <pod> -n <namespace> | tail -20",
    },

    // ── Security ──
    KnownPattern {
        id: "permission-denied",
        category: Category::Security,
        severity: Severity::Error,
        keywords: &["permission denied", "EACCES", "Forbidden", "RBAC"],
        explanation: "A process or pod was denied access to a resource. This may be \
                      a filesystem permission issue or a Kubernetes RBAC policy.",
        fix: "For K8s RBAC: check the ServiceAccount's ClusterRole bindings.\n\
              For filesystem: check the container's securityContext and fsGroup settings.",
        investigate: "kubectl auth can-i <verb> <resource> --as=system:serviceaccount:<ns>:<sa>",
    },

    // ── Cloudflare ──
    KnownPattern {
        id: "cloudflare-tunnel-disconnect",
        category: Category::Network,
        severity: Severity::Warning,
        keywords: &["connection with edge closed", "cloudflared", "Unable to reach the origin"],
        explanation: "Cloudflare tunnel connections are dropping. This causes intermittent \
                      502 errors for services behind the tunnel. Usually caused by network \
                      instability on the host.",
        fix: "Check host network stability (DHCP, WiFi, route changes).\n\
              If persistent, restart cloudflared:\n\
              kubectl -n cloudflared rollout restart deployment cloudflared",
        investigate: "Check if the tunnel reconnects automatically. Occasional disconnects \
                     are normal. Frequent disconnects indicate a network issue on the host.",
    },

    // ── NixOS specific ──
    KnownPattern {
        id: "nix-gc-failed",
        category: Category::Disk,
        severity: Severity::Warning,
        keywords: &["nix-gc.service", "removeOldGenerations"],
        explanation: "Nix garbage collection failed. Old store paths are not being cleaned up, \
                      which will eventually fill the disk.",
        fix: "Run garbage collection manually:\n\
              sudo nix-collect-garbage -d\n\
              Then check why the service failed:\n\
              journalctl -u nix-gc.service",
        investigate: "Check disk space in /nix/store:\n\
                     du -sh /nix/store",
    },
];

/// Match a log template or error message against the knowledge base.
/// Returns all matching patterns, sorted by severity (critical first).
pub fn match_patterns(text: &str) -> Vec<&'static KnownPattern> {
    let lower = text.to_lowercase();
    let mut matches: Vec<&KnownPattern> = PATTERNS
        .iter()
        .filter(|p| p.keywords.iter().any(|kw| lower.contains(&kw.to_lowercase())))
        .collect();
    matches.sort_by(|a, b| b.severity.partial_cmp(&a.severity).unwrap());
    matches
}

/// Given a list of log events (template_text, count, severity), produce
/// a diagnostic report with explanations and fixes.
pub fn diagnose_events(events: &[(String, i64, String)]) -> Vec<Diagnosis> {
    let mut diagnoses = Vec::new();

    for (template, count, severity) in events {
        let patterns = match_patterns(template);
        if let Some(pattern) = patterns.first() {
            diagnoses.push(Diagnosis {
                pattern_id: pattern.id,
                category: pattern.category,
                severity: pattern.severity,
                template: template.clone(),
                occurrences: *count,
                log_severity: severity.clone(),
                explanation: pattern.explanation,
                fix: pattern.fix,
                investigate: pattern.investigate,
            });
        }
    }

    // Deduplicate by pattern_id (same root cause may appear in multiple templates)
    diagnoses.sort_by(|a, b| b.severity.partial_cmp(&a.severity).unwrap());
    diagnoses.dedup_by(|a, b| a.pattern_id == b.pattern_id);
    diagnoses
}

pub struct Diagnosis {
    pub pattern_id: &'static str,
    pub category: Category,
    pub severity: Severity,
    pub template: String,
    pub occurrences: i64,
    pub log_severity: String,
    pub explanation: &'static str,
    pub fix: &'static str,
    pub investigate: &'static str,
}

impl Diagnosis {
    pub fn format(&self) -> String {
        let sev_icon = match self.severity {
            Severity::Critical => "🔴",
            Severity::Error => "🟠",
            Severity::Warning => "🟡",
            Severity::Info => "🔵",
        };
        let cat = format!("{:?}", self.category).to_uppercase();
        format!(
            "{} {} [{}] ({} occurrences)\n\n\
             {}\n\n\
             Fix:\n{}\n\n\
             If that doesn't work:\n{}",
            sev_icon, cat, self.log_severity, self.occurrences,
            self.explanation, self.fix, self.investigate,
        )
    }
}
