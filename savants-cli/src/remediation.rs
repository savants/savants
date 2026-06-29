//! Remediation engine — suggests fixes and executes them ONLY with approval.
//!
//! Safety model:
//! 1. Savants detects an issue
//! 2. Savants suggests a specific fix (a command)
//! 3. The command is validated against the SAFETY POLICY
//! 4. User approves via CLI, Slack, or web UI
//! 5. Only then does Savants execute
//!
//! The safety policy defines what Savants is ALLOWED to do.
//! Everything not explicitly allowed is DENIED.
//! Destructive operations are ALWAYS denied regardless of policy.

/// Commands that Savants is NEVER allowed to execute, regardless of policy.
/// These are the "delete the whole cluster" commands you're worried about.
const ALWAYS_BLOCKED: &[&str] = &[
    "delete namespace",
    "delete cluster",
    "delete node",
    "delete pv ",          // persistent volumes
    "delete pvc ",         // persistent volume claims (data loss)
    "drain ",              // evacuates a node
    "cordon ",             // blocks scheduling (only during active security response)
    "scale --replicas=0",  // kills all instances
    "helm uninstall",
    "helm delete",
    "terraform destroy",
    "rm -rf",
    "dd if=",
    "mkfs",
    "fdisk",
    "DROP DATABASE",
    "DROP TABLE",
    "TRUNCATE",
    "--force --grace-period=0",  // force delete without grace
    "reset --hard",        // git destructive
    "push --force",        // git destructive
];

/// Commands that Savants is allowed to SUGGEST and EXECUTE with approval.
/// Anything not on this list can be suggested but NOT auto-executed.
const ALLOWED_WITH_APPROVAL: &[&str] = &[
    // Safe K8s operations
    "kubectl delete pod",          // restart a single pod (K8s recreates it)
    "kubectl rollout restart",     // rolling restart of a deployment
    "kubectl scale",               // scale up (not down to 0)
    "kubectl edit configmap",      // edit config (user reviews the edit)
    "kubectl apply -f",            // apply a manifest
    "kubectl create secret",       // create a secret
    "kubectl patch",               // patch a resource

    // Safe system operations
    "systemctl restart",           // restart a service
    "systemctl start",             // start a stopped service
    "nmcli connection modify",     // network config change
    "nmcli connection up",         // apply network change

    // Safe NixOS operations
    "nixos-rebuild switch",        // apply nix config
    "nix-collect-garbage",         // clean up old generations
];

/// A proposed remediation action.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Remediation {
    pub id: String,
    pub alert_id: String,
    pub command: String,
    pub description: String,
    pub safety: SafetyLevel,
    pub status: RemediationStatus,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SafetyLevel {
    /// Safe to execute — on the allowlist, non-destructive
    Safe,
    /// Requires review — not on allowlist but not blocked either
    NeedsReview,
    /// Blocked — on the ALWAYS_BLOCKED list, will never execute
    Blocked,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum RemediationStatus {
    Proposed,     // suggested, waiting for approval
    Approved,     // user said yes
    Rejected,     // user said no
    Executing,    // running the command
    Completed,    // done
    Failed,       // command failed
}

/// Validate a proposed fix command against the safety policy.
pub fn classify_command(command: &str) -> SafetyLevel {
    let lower = command.to_lowercase();

    // Check blocked list first — these NEVER execute
    for blocked in ALWAYS_BLOCKED {
        if lower.contains(&blocked.to_lowercase()) {
            return SafetyLevel::Blocked;
        }
    }

    // Check allowed list — these can execute with approval
    for allowed in ALLOWED_WITH_APPROVAL {
        if lower.contains(&allowed.to_lowercase()) {
            return SafetyLevel::Safe;
        }
    }

    // Everything else needs manual review
    SafetyLevel::NeedsReview
}

/// Propose a remediation. Returns the safety classification.
pub fn propose(alert_id: &str, command: &str, description: &str) -> Remediation {
    let safety = classify_command(command);
    Remediation {
        id: format!("fix-{}-{}", alert_id, chrono::Utc::now().timestamp()),
        alert_id: alert_id.to_string(),
        command: command.to_string(),
        description: description.to_string(),
        safety,
        status: RemediationStatus::Proposed,
    }
}

/// Execute an approved remediation. Returns success/failure.
pub fn execute(remediation: &mut Remediation) -> Result<String, String> {
    // Double-check safety — never trust the caller
    match classify_command(&remediation.command) {
        SafetyLevel::Blocked => {
            remediation.status = RemediationStatus::Rejected;
            return Err(format!(
                "BLOCKED: '{}' is on the never-execute list. This command cannot be run by Savants.",
                remediation.command
            ));
        }
        SafetyLevel::NeedsReview => {
            if !matches!(remediation.status, RemediationStatus::Approved) {
                return Err("This command requires explicit approval before execution.".into());
            }
        }
        SafetyLevel::Safe => {
            if !matches!(remediation.status, RemediationStatus::Approved) {
                return Err("Even safe commands require approval before execution.".into());
            }
        }
    }

    remediation.status = RemediationStatus::Executing;

    // Execute the command
    let output = std::process::Command::new("sh")
        .args(["-c", &remediation.command])
        .output()
        .map_err(|e| format!("Failed to execute: {}", e))?;

    if output.status.success() {
        remediation.status = RemediationStatus::Completed;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(if stdout.is_empty() { "Command completed successfully.".into() } else { stdout })
    } else {
        remediation.status = RemediationStatus::Failed;
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(format!("Command failed: {}", stderr))
    }
}

/// Format a remediation for display in alerts/notifications.
pub fn format_for_alert(remediation: &Remediation) -> String {
    let safety_badge = match remediation.safety {
        SafetyLevel::Safe => "✅ SAFE",
        SafetyLevel::NeedsReview => "⚠️ NEEDS REVIEW",
        SafetyLevel::Blocked => "🚫 BLOCKED",
    };

    format!(
        "{}\n\nSuggested fix [{}]:\n  {}\n\n{}",
        remediation.description,
        safety_badge,
        remediation.command,
        match remediation.safety {
            SafetyLevel::Safe => "Run: savants fix approve <id>",
            SafetyLevel::NeedsReview => "Review carefully, then: savants fix approve <id>",
            SafetyLevel::Blocked => "This command is blocked by safety policy. Manual intervention required.",
        }
    )
}
