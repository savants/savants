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
        explanation: "WiFi adapter is dropping a high number of packets. On 2.4 GHz, \
                      this is almost always interference from other devices on the same \
                      channel (neighbors' routers, Bluetooth, microwaves). On 5 GHz, \
                      it may indicate weak signal or driver issues.",
        fix: "1. Switch to 5 GHz: nmcli connection modify <name> wifi.band a\n\
              2. Disable power save: 802-11-wireless.powersave 2\n\
              3. Apply: nmcli connection up <name>\n\
              4. Best fix: use an ethernet cable for servers.",
        investigate: "Check packet stats: cat /proc/net/wireless\n\
                     Check channel congestion: nmcli dev wifi list\n\
                     Check power save: iwconfig <iface> | grep Power",
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
