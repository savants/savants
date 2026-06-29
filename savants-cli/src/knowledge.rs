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
    // ── Cost / Resource waste ──
    KnownPattern {
        id: "pod-pending-unschedulable",
        category: Category::Performance,
        severity: Severity::Warning,
        keywords: &["Unschedulable", "Insufficient cpu", "Insufficient memory",
                    "didn't match Pod's node affinity"],
        explanation: "A pod cannot be scheduled because the cluster lacks sufficient \
                      resources or the pod's affinity rules don't match any node. \
                      You may be paying for capacity that can't be used.",
        fix: "Check node capacity vs requests:\n\
              kubectl describe nodes | grep -A 5 'Allocated resources'\n\
              Consider: reduce resource requests, add nodes, or relax affinity rules.",
        investigate: "kubectl get events --field-selector reason=FailedScheduling",
    },
    KnownPattern {
        id: "resource-limits-missing",
        category: Category::Performance,
        severity: Severity::Warning,
        keywords: &["no resource limits", "LimitRange", "requests.cpu: 0",
                    "requests.memory: 0"],
        explanation: "Containers without resource limits can consume unbounded CPU/memory, \
                      starving other workloads and making costs unpredictable.",
        fix: "Add resource limits to all containers:\n\
              resources:\n\
                requests: { cpu: '100m', memory: '128Mi' }\n\
                limits: { cpu: '500m', memory: '512Mi' }",
        investigate: "Find all pods without limits:\n\
                     kubectl get pods -A -o json | jq '.items[] | select(.spec.containers[].resources.limits == null) | .metadata.name'",
    },

    // ── Vulnerabilities / Security ──
    KnownPattern {
        id: "image-pull-backoff",
        category: Category::Security,
        severity: Severity::Error,
        keywords: &["ImagePullBackOff", "ErrImagePull", "unauthorized",
                    "authentication required"],
        explanation: "A container image cannot be pulled. Either the image doesn't exist, \
                      the tag is wrong, or the registry credentials are missing/expired.",
        fix: "Check the image name and tag:\n\
              kubectl describe pod <name> | grep Image\n\
              If it's a private registry, ensure imagePullSecrets are configured:\n\
              kubectl get secrets -n <namespace> | grep docker",
        investigate: "Try pulling the image manually:\n\
                     docker pull <image>:<tag>",
    },
    KnownPattern {
        id: "privileged-container",
        category: Category::Security,
        severity: Severity::Warning,
        keywords: &["privileged: true", "SYS_ADMIN", "hostPID: true",
                    "hostNetwork: true"],
        explanation: "A container is running with elevated privileges. This is a \
                      security risk — a compromised container could access the host.",
        fix: "Remove privileged mode unless absolutely necessary:\n\
              securityContext:\n\
                privileged: false\n\
                runAsNonRoot: true\n\
                readOnlyRootFilesystem: true",
        investigate: "Find all privileged pods:\n\
                     kubectl get pods -A -o json | jq '.items[] | select(.spec.containers[].securityContext.privileged == true)'",
    },
    KnownPattern {
        id: "exposed-secret-in-env",
        category: Category::Security,
        severity: Severity::Critical,
        keywords: &["password", "secret", "token", "api_key", "API_KEY",
                    "SECRET_KEY", "PRIVATE_KEY"],
        explanation: "A secret value may be exposed in environment variables or logs. \
                      Secrets should be mounted as files, not passed as env vars.",
        fix: "Use Kubernetes Secrets mounted as volumes instead of env vars:\n\
              volumeMounts:\n\
                - name: secrets\n\
                  mountPath: /etc/secrets\n\
                  readOnly: true",
        investigate: "Check if secrets are in pod env:\n\
                     kubectl get pod <name> -o json | jq '.spec.containers[].env[] | select(.valueFrom.secretKeyRef)'",
    },

    // ── Rate limits / Throttling ──
    KnownPattern {
        id: "rate-limited-429",
        category: Category::Performance,
        severity: Severity::Warning,
        keywords: &["429", "Too Many Requests", "rate limit", "throttl",
                    "Retry-After", "quota exceeded"],
        explanation: "An API is returning 429 Too Many Requests. The application is \
                      being throttled. This causes latency spikes and failures.",
        fix: "Implement exponential backoff in the client.\n\
              Check if you're hitting API quota limits and request an increase.\n\
              Consider caching responses to reduce call frequency.",
        investigate: "Check which endpoint is being throttled:\n\
                     Look for the full URL in the log template.",
    },
    KnownPattern {
        id: "api-server-throttled",
        category: Category::Performance,
        severity: Severity::Warning,
        keywords: &["Throttling request", "too many requests",
                    "client-side throttling", "request throttled"],
        explanation: "The Kubernetes API server is throttling requests from a client. \
                      A controller or operator may be making too many API calls.",
        fix: "Identify the client making excessive calls:\n\
              kubectl get --raw /metrics | grep apiserver_request_total\n\
              Consider increasing API server QPS limits or fixing the chatty controller.",
        investigate: "Check which service account is making the most requests.",
    },

    // ── DDoS / Attacks ──
    KnownPattern {
        id: "brute-force-auth",
        category: Category::Security,
        severity: Severity::Critical,
        keywords: &["authentication failed", "invalid password", "login failed",
                    "unauthorized", "403 Forbidden", "brute force",
                    "too many authentication failures"],
        explanation: "Multiple authentication failures detected. This may indicate \
                      a brute-force attack or misconfigured credentials.",
        fix: "If external-facing: enable rate limiting on the auth endpoint.\n\
              Consider fail2ban or similar for SSH.\n\
              Check if it's a misconfigured service account (internal).",
        investigate: "Check the source IPs of failed auth attempts.\n\
                     If they're from a single IP, block it at the firewall.",
    },
    KnownPattern {
        id: "connection-flood",
        category: Category::Security,
        severity: Severity::Critical,
        keywords: &["SYN flood", "too many open files", "EMFILE", "ENFILE",
                    "socket: too many open files", "accept4: too many open files"],
        explanation: "The system is running out of file descriptors due to too many \
                      concurrent connections. This may be a DDoS attack or a connection leak.",
        fix: "Increase file descriptor limits:\n\
              ulimit -n 65535\n\
              Or in systemd: LimitNOFILE=65535\n\
              If it's an attack, enable SYN cookies and rate limiting.",
        investigate: "Check connection count per source:\n\
                     ss -s  # summary\n\
                     ss -tn | awk '{print $5}' | sort | uniq -c | sort -rn | head",
    },
    KnownPattern {
        id: "unusual-process",
        category: Category::Security,
        severity: Severity::Warning,
        keywords: &["cryptominer", "xmrig", "kdevtmpfsi", "kinsing",
                    "suspicious process", "reverse shell"],
        explanation: "A potentially malicious process was detected. This may indicate \
                      a compromised container or host.",
        fix: "Immediately isolate the affected pod/host:\n\
              kubectl cordon <node>  # prevent new pods\n\
              kubectl delete pod <name> --force  # kill the pod\n\
              Investigate how the attacker gained access.",
        investigate: "Check process tree: ps auxf\n\
                     Check network connections: ss -tlnp\n\
                     Check container image for known vulnerabilities.",
    },

    // ── Certificate ──
    KnownPattern {
        id: "cert-expiring",
        category: Category::Certificate,
        severity: Severity::Warning,
        keywords: &["certificate expired", "x509: certificate has expired",
                    "certificate is not yet valid", "tls: bad certificate"],
        explanation: "A TLS certificate has expired or is invalid. HTTPS connections \
                      to this service will fail.",
        fix: "Check cert-manager for failed certificate renewals:\n\
              kubectl get certificates -A\n\
              kubectl describe certificate <name> -n <namespace>\n\
              Force renewal: kubectl delete certificate <name> -n <namespace>",
        investigate: "Check the actual certificate expiry:\n\
                     echo | openssl s_client -connect <host>:443 2>/dev/null | openssl x509 -noout -dates",
    },
    // ── WiFi / Network quality ──
    KnownPattern {
        id: "wifi-high-packet-discard",
        category: Category::Network,
        severity: Severity::Warning,
        keywords: &["WiFi", "discarding", "packets", "2.4GHz", "interference"],
        explanation: "WiFi adapter is dropping a high number of packets. Common causes:\n\
                      - On 2.4 GHz: interference from other devices (routers, Bluetooth, microwaves)\n\
                      - On 5 GHz: weak signal (shorter range) or driver issues\n\
                      - On any band: power management causing micro-disconnects\n\
                      - Hardware: failing WiFi adapter or antenna",
        fix: "Diagnose first, then fix:\n\
              1. Check what band you're on: nmcli dev wifi list | grep '*'\n\
              2. If 2.4 GHz and router supports 5 GHz: nmcli connection modify <name> wifi.band a\n\
              3. If 2.4 GHz and NO 5 GHz available: change to a less congested channel on the router\n\
              4. Disable power save: nmcli connection modify <name> 802-11-wireless.powersave 2\n\
              5. Apply changes: nmcli connection up <name>\n\
              6. Best for servers: use an ethernet cable — eliminates all WiFi issues",
        investigate: "Check packet stats: cat /proc/net/wireless\n\
                     Check channel congestion: nmcli dev wifi list\n\
                     Check available bands: nmcli dev wifi list | awk '{print $5}' | sort -u\n\
                     Check power save: cat /sys/module/iwlmvm/parameters/power_scheme",
    },
    KnownPattern {
        id: "wifi-weak-signal",
        category: Category::Network,
        severity: Severity::Warning,
        keywords: &["signal", "dBm", "weak", "wifi", "-80", "-85", "-90"],
        explanation: "WiFi signal is weak (below -70 dBm). This causes packet loss, \
                      retransmissions, and intermittent connectivity drops.",
        fix: "Move closer to the access point, or use a WiFi extender.\n\
              For servers: use an ethernet cable — WiFi is unreliable for production.",
        investigate: "Check signal: cat /proc/net/wireless\n\
                     Check which band: nmcli dev wifi list | grep '*'",
    },
    KnownPattern {
        id: "wifi-power-save",
        category: Category::Performance,
        severity: Severity::Warning,
        keywords: &["power_save", "powersave", "Power Management:on"],
        explanation: "WiFi power management is enabled. The adapter periodically sleeps \
                      to save battery, causing micro-disconnects. This is fine for laptops \
                      but terrible for servers.",
        fix: "Disable power save:\n\
              nmcli connection modify <name> 802-11-wireless.powersave 2\n\
              nmcli connection up <name>",
        investigate: "Check current state: cat /sys/module/iwlmvm/parameters/power_scheme",
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

// ─────────────────────────────────────────────────────────────────────────────
// v2 Dynamic Diagnosis Engine
// ─────────────────────────────────────────────────────────────────────────────
//
// The static patterns above detect WHAT is wrong.
// The dynamic engine below queries the graph to understand WHY and HOW to fix
// it based on the actual environment.

use crate::graph::GraphClient;

/// Contextual data pulled from the graph for a specific host/cluster.
pub struct DiagnosisContext {
    pub wifi_band: Option<String>,
    pub wifi_channel: Option<i64>,
    pub wifi_5ghz_available: bool,
    pub has_ethernet: bool,
    pub disk_mounts: Vec<DiskMount>,
    pub dns_system_resolver: Option<String>,
    pub dns_system_response_ms: Option<f64>,
    pub dns_alt_response_ms: Option<f64>,
    pub k8s_cluster_healthy: bool,
    pub dns_resolving: bool,
    pub crashloop_pods: Vec<CrashloopPod>,
}

pub struct DiskMount {
    pub mountpoint: String,
    pub percent: f64,
}

pub struct CrashloopPod {
    pub name: String,
    pub namespace: String,
    pub restart_count: i64,
    pub caused_by: Vec<String>,
}

/// A diagnosis enriched with graph-sourced context and a dynamically generated fix.
pub struct DynamicDiagnosis {
    pub pattern: &'static KnownPattern,
    pub context: DiagnosisContext,
    pub suggested_fix: String,
    pub template: String,
    pub occurrences: i64,
    pub log_severity: String,
}

impl DynamicDiagnosis {
    pub fn format(&self) -> String {
        let sev_icon = match self.pattern.severity {
            Severity::Critical => "🔴",
            Severity::Error => "🟠",
            Severity::Warning => "🟡",
            Severity::Info => "🔵",
        };
        let cat = format!("{:?}", self.pattern.category).to_uppercase();
        format!(
            "{} {} [{}] ({} occurrences)\n\n\
             {}\n\n\
             Context-aware fix:\n{}\n\n\
             Static fallback fix:\n{}\n\n\
             If that doesn't work:\n{}",
            sev_icon, cat, self.log_severity, self.occurrences,
            self.pattern.explanation, self.suggested_fix,
            self.pattern.fix, self.pattern.investigate,
        )
    }
}

impl Default for DiagnosisContext {
    fn default() -> Self {
        Self {
            wifi_band: None,
            wifi_channel: None,
            wifi_5ghz_available: false,
            has_ethernet: false,
            disk_mounts: Vec::new(),
            dns_system_resolver: None,
            dns_system_response_ms: None,
            dns_alt_response_ms: None,
            k8s_cluster_healthy: true,
            dns_resolving: true,
            crashloop_pods: Vec::new(),
        }
    }
}

// ── Graph query helpers ──────────────────────────────────────────────────────

/// Query WiFi context from the graph.
fn query_wifi_context(client: &GraphClient) -> DiagnosisContext {
    let mut ctx = DiagnosisContext::default();

    // Current WiFi status
    if let Ok(result) = client.query(
        "MATCH (w:WifiStatus) RETURN w.band, w.channel",
        &[],
    ) {
        if let Some(row) = result.rows.first() {
            if row.len() >= 2 {
                let band = row[0].as_str().to_string();
                if !band.is_empty() {
                    ctx.wifi_band = Some(band);
                }
                let ch = row[1].as_i64();
                if ch > 0 {
                    ctx.wifi_channel = Some(ch);
                }
            }
        }
    }

    // Check if 5 GHz networks are available
    if let Ok(result) = client.query(
        "MATCH (n:WifiNetwork) WHERE n.band = '5GHz' OR n.frequency > 5000 RETURN count(n)",
        &[],
    ) {
        if let Some(row) = result.rows.first() {
            ctx.wifi_5ghz_available = row.first().map(|v| v.as_i64() > 0).unwrap_or(false);
        }
    }

    // Check for ethernet
    if let Ok(result) = client.query(
        "MATCH (i:NetworkInterface) WHERE i.type = 'ethernet' AND i.state = 'up' RETURN count(i)",
        &[],
    ) {
        if let Some(row) = result.rows.first() {
            ctx.has_ethernet = row.first().map(|v| v.as_i64() > 0).unwrap_or(false);
        }
    }

    ctx
}

/// Query disk context from the graph.
fn query_disk_context(client: &GraphClient) -> DiagnosisContext {
    let mut ctx = DiagnosisContext::default();

    if let Ok(result) = client.query(
        "MATCH (d:HostDisk) RETURN d.mountpoint, d.percent ORDER BY d.percent DESC",
        &[],
    ) {
        for row in &result.rows {
            if row.len() >= 2 {
                let mountpoint = row[0].as_str().to_string();
                let percent = row[1].as_f64();
                if !mountpoint.is_empty() {
                    ctx.disk_mounts.push(DiskMount { mountpoint, percent });
                }
            }
        }
    }

    ctx
}

/// Query DNS context from the graph.
fn query_dns_context(client: &GraphClient) -> DiagnosisContext {
    let mut ctx = DiagnosisContext::default();

    if let Ok(result) = client.query(
        "MATCH (d:DnsCheck) RETURN d.resolver, d.response_time_ms ORDER BY d.response_time_ms ASC",
        &[],
    ) {
        for row in &result.rows {
            if row.len() >= 2 {
                let resolver = row[0].as_str().to_string();
                let response_ms = row[1].as_f64();

                // Categorize: system resolver vs well-known public resolvers
                if resolver == "1.1.1.1" || resolver == "8.8.8.8" || resolver == "9.9.9.9" {
                    // Keep the fastest alt resolver
                    if ctx.dns_alt_response_ms.is_none()
                        || response_ms < ctx.dns_alt_response_ms.unwrap_or(f64::MAX)
                    {
                        ctx.dns_alt_response_ms = Some(response_ms);
                    }
                } else if ctx.dns_system_resolver.is_none() {
                    ctx.dns_system_resolver = Some(resolver);
                    ctx.dns_system_response_ms = Some(response_ms);
                }
            }
        }

        // If both are slow (> 500ms), it's a network issue
        let sys_slow = ctx.dns_system_response_ms.map(|ms| ms > 200.0).unwrap_or(false);
        let alt_slow = ctx.dns_alt_response_ms.map(|ms| ms > 200.0).unwrap_or(true);
        ctx.dns_resolving = !(sys_slow && alt_slow);
    }

    ctx
}

/// Query CrashLoopBackOff pod context from the graph.
fn query_crashloop_context(client: &GraphClient) -> DiagnosisContext {
    let mut ctx = DiagnosisContext::default();

    if let Ok(result) = client.query(
        "MATCH (p:K8sPod) WHERE p.status = 'CrashLoopBackOff' \
         RETURN p.name, p.namespace, p.restart_count ORDER BY p.restart_count DESC",
        &[],
    ) {
        for row in &result.rows {
            if row.len() >= 3 {
                let name = row[0].as_str().to_string();
                let namespace = row[1].as_str().to_string();
                let restart_count = row[2].as_i64();

                // Query CAUSED_BY edges for temporal correlation
                let mut caused_by = Vec::new();
                if let Ok(cause_result) = client.query(
                    &format!(
                        "MATCH (p:K8sPod {{name: '{}'}})-[:CAUSED_BY]->(e) \
                         RETURN labels(e), e.description, e.template \
                         LIMIT 5",
                        name.replace('\'', "\\'")
                    ),
                    &[],
                ) {
                    for cause_row in &cause_result.rows {
                        let desc = if cause_row.len() >= 3 {
                            let d = cause_row[1].as_str();
                            let t = cause_row[2].as_str();
                            if !d.is_empty() {
                                d.to_string()
                            } else {
                                t.to_string()
                            }
                        } else if !cause_row.is_empty() {
                            cause_row[0].as_str().to_string()
                        } else {
                            continue;
                        };
                        if !desc.is_empty() {
                            caused_by.push(desc);
                        }
                    }
                }

                ctx.crashloop_pods.push(CrashloopPod {
                    name,
                    namespace,
                    restart_count,
                    caused_by,
                });
            }
        }
    }

    ctx
}

// ── Dynamic fix generators ───────────────────────────────────────────────────

fn generate_wifi_fix(ctx: &DiagnosisContext) -> String {
    let mut fix = String::new();

    match ctx.wifi_band.as_deref() {
        Some(band) if band.contains("2.4") || band == "bg" || band == "bgn" => {
            if ctx.wifi_5ghz_available {
                fix.push_str(&format!(
                    "You are on 2.4 GHz (channel {}). A 5 GHz network is available.\n\
                     Switch to 5 GHz for less interference and higher throughput:\n\
                     nmcli connection modify <name> wifi.band a\n\
                     nmcli connection up <name>",
                    ctx.wifi_channel.map(|c| c.to_string()).unwrap_or_else(|| "unknown".into())
                ));
            } else {
                fix.push_str(&format!(
                    "You are on 2.4 GHz (channel {}) and no 5 GHz network is available.\n\
                     Change the channel on your router to reduce interference.\n\
                     Current channel: {}. Recommended non-overlapping channels: 1, 6, or 11.\n\
                     Pick whichever has the least congestion (check: nmcli dev wifi list).",
                    ctx.wifi_channel.map(|c| c.to_string()).unwrap_or_else(|| "unknown".into()),
                    ctx.wifi_channel.map(|c| c.to_string()).unwrap_or_else(|| "unknown".into()),
                ));
            }
        }
        Some(band) if band.contains("5") || band == "a" || band == "ac" || band == "ax" => {
            fix.push_str(
                "You are on 5 GHz. Signal may be weak due to shorter range.\n\
                 Check signal strength: cat /proc/net/wireless\n\
                 If link quality is low, move closer to the access point or add a repeater.\n\
                 Consider a wired ethernet connection for stability."
            );
        }
        Some(band) => {
            fix.push_str(&format!(
                "Detected WiFi band: {}. Check signal quality: cat /proc/net/wireless\n\
                 If packet discard rate is high, consider switching bands or channels.",
                band
            ));
        }
        None => {
            fix.push_str(
                "Could not determine WiFi band from graph. Check manually:\n\
                 nmcli dev wifi list | grep '*'\n\
                 If on 2.4 GHz and 5 GHz is available, switch to 5 GHz."
            );
        }
    }

    if ctx.has_ethernet {
        fix.push_str("\n\nEthernet is connected on this host. For maximum reliability, \
                      switch the primary workload traffic to the wired interface.");
    }

    fix.push_str("\n\nFor servers, ethernet is the most reliable option.");
    fix
}

fn generate_disk_fix(ctx: &DiagnosisContext) -> String {
    let mut fix = String::new();

    if ctx.disk_mounts.is_empty() {
        fix.push_str(
            "Could not query disk data from graph. Check manually:\n\
             df -h\n\
             du -sh /* | sort -rh | head -20"
        );
        return fix;
    }

    for mount in &ctx.disk_mounts {
        if mount.percent > 90.0 {
            fix.push_str(&format!(
                "CRITICAL: {} is at {:.1}%. Disk is nearly full.\n\
                 Immediate actions:\n\
                 du -sh {}/* 2>/dev/null | sort -rh | head -20\n\
                 journalctl --vacuum-size=100M\n\
                 docker system prune -f  # if Docker is present\n\
                 crictl rmi --prune       # if containerd/CRI is present\n\
                 find {} -name '*.log' -size +100M -exec truncate -s 0 {{}} \\;\n",
                mount.mountpoint, mount.percent, mount.mountpoint, mount.mountpoint,
            ));
        } else if mount.percent > 80.0 {
            fix.push_str(&format!(
                "WARNING: {} is at {:.1}%, approaching full.\n\
                 Investigate top space consumers:\n\
                 du -sh {}/* 2>/dev/null | sort -rh | head -20\n\
                 Consider setting up log rotation and container image pruning.\n",
                mount.mountpoint, mount.percent, mount.mountpoint,
            ));
        } else {
            fix.push_str(&format!(
                "{} is at {:.1}% (healthy).\n",
                mount.mountpoint, mount.percent,
            ));
        }
    }

    fix
}

fn generate_dns_fix(ctx: &DiagnosisContext) -> String {
    let mut fix = String::new();

    let sys_ms = ctx.dns_system_response_ms;
    let alt_ms = ctx.dns_alt_response_ms;
    let sys_resolver = ctx.dns_system_resolver.as_deref().unwrap_or("unknown");

    match (sys_ms, alt_ms) {
        (Some(sys), Some(alt)) if sys > 200.0 && alt < 100.0 => {
            fix.push_str(&format!(
                "System DNS ({}) is slow: {:.0}ms. Public DNS (1.1.1.1) is fast: {:.0}ms.\n\
                 Your DNS resolver is the bottleneck. Switch to a faster resolver:\n\
                 For systemd-resolved:\n\
                   sudo mkdir -p /etc/systemd/resolved.conf.d\n\
                   echo -e '[Resolve]\\nDNS=1.1.1.1 8.8.8.8' | sudo tee /etc/systemd/resolved.conf.d/dns.conf\n\
                   sudo systemctl restart systemd-resolved\n\
                 For /etc/resolv.conf:\n\
                   echo 'nameserver 1.1.1.1' | sudo tee /etc/resolv.conf",
                sys_resolver, sys, alt,
            ));
        }
        (Some(sys), Some(alt)) if sys > 200.0 && alt > 200.0 => {
            fix.push_str(&format!(
                "Both system DNS ({}: {:.0}ms) and public DNS (1.1.1.1: {:.0}ms) are slow.\n\
                 This is a network-level issue, not a DNS configuration problem.\n\
                 Check: is the host's uplink saturated? Is there packet loss?\n\
                 ping -c 10 1.1.1.1  # check for packet loss\n\
                 mtr 1.1.1.1         # trace the path",
                sys_resolver, sys, alt,
            ));
        }
        (Some(sys), _) if sys < 100.0 => {
            fix.push_str(&format!(
                "System DNS ({}) response time is {:.0}ms (healthy).\n\
                 DNS resolution is working. The issue may be intermittent or resolved.",
                sys_resolver, sys,
            ));
        }
        _ => {
            fix.push_str(
                "Could not determine DNS performance from graph. Check manually:\n\
                 dig @127.0.0.53 example.com  # system resolver\n\
                 dig @1.1.1.1 example.com     # public resolver\n\
                 Compare response times to isolate the issue."
            );
        }
    }

    fix
}

fn generate_crashloop_fix(ctx: &DiagnosisContext) -> String {
    let mut fix = String::new();

    if ctx.crashloop_pods.is_empty() {
        fix.push_str(
            "No CrashLoopBackOff pods found in graph. The issue may have self-resolved.\n\
             Check current state: kubectl get pods -A | grep CrashLoopBackOff"
        );
        return fix;
    }

    fix.push_str(&format!(
        "{} pod(s) in CrashLoopBackOff:\n\n",
        ctx.crashloop_pods.len()
    ));

    for pod in &ctx.crashloop_pods {
        fix.push_str(&format!(
            "  {} (namespace: {}, restarts: {})\n",
            pod.name, pod.namespace, pod.restart_count
        ));

        if !pod.caused_by.is_empty() {
            fix.push_str("    Correlated causes (CAUSED_BY edges):\n");
            for cause in &pod.caused_by {
                fix.push_str(&format!("    - {}\n", cause));
            }
        }

        fix.push_str(&format!(
            "    Investigate:\n\
             kubectl logs {} -n {} --previous\n\
             kubectl describe pod {} -n {}\n\n",
            pod.name, pod.namespace, pod.name, pod.namespace,
        ));
    }

    if ctx.crashloop_pods.len() > 3 {
        fix.push_str(
            "Multiple pods crashing simultaneously often indicates a shared root cause:\n\
             DNS failure, node resource pressure, or a bad config change.\n\
             Check node conditions: kubectl describe nodes | grep -A5 Conditions"
        );
    }

    fix
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Match events against static patterns, then query the graph for each match
/// to produce context-aware dynamic diagnoses.
///
/// This is the v2 replacement for `diagnose_events`. The static `diagnose_events`
/// function still works and is not modified — this function is an addition.
pub fn diagnose_with_context(
    events: &[(String, i64, String)],
    client: &GraphClient,
) -> Vec<DynamicDiagnosis> {
    let mut diagnoses = Vec::new();

    for (template, count, severity) in events {
        let patterns = match_patterns(template);
        if let Some(pattern) = patterns.first() {
            let (ctx, suggested_fix) = build_dynamic_diagnosis(pattern, client);

            diagnoses.push(DynamicDiagnosis {
                pattern,
                context: ctx,
                suggested_fix,
                template: template.clone(),
                occurrences: *count,
                log_severity: severity.clone(),
            });
        }
    }

    // Deduplicate by pattern id, keeping higher severity
    diagnoses.sort_by(|a, b| b.pattern.severity.partial_cmp(&a.pattern.severity).unwrap());
    diagnoses.dedup_by(|a, b| a.pattern.id == b.pattern.id);
    diagnoses
}

/// For a single matched pattern, query the graph and generate a context-aware fix.
fn build_dynamic_diagnosis(
    pattern: &'static KnownPattern,
    client: &GraphClient,
) -> (DiagnosisContext, String) {
    match pattern.category {
        Category::Network if is_wifi_pattern(pattern) => {
            let ctx = query_wifi_context(client);
            let fix = generate_wifi_fix(&ctx);
            (ctx, fix)
        }
        Category::Disk => {
            let ctx = query_disk_context(client);
            let fix = generate_disk_fix(&ctx);
            (ctx, fix)
        }
        Category::Dns => {
            let ctx = query_dns_context(client);
            let fix = generate_dns_fix(&ctx);
            (ctx, fix)
        }
        Category::Crash if pattern.id == "crash-loop-backoff" => {
            let ctx = query_crashloop_context(client);
            let fix = generate_crashloop_fix(&ctx);
            (ctx, fix)
        }
        _ => {
            // No dynamic enrichment for this category yet — return static fix
            let ctx = DiagnosisContext::default();
            let fix = pattern.fix.to_string();
            (ctx, fix)
        }
    }
}

/// Check whether a pattern is WiFi-related (within the Network category).
fn is_wifi_pattern(pattern: &KnownPattern) -> bool {
    matches!(
        pattern.id,
        "wifi-high-packet-discard" | "wifi-weak-signal" | "wifi-power-save"
    )
}
