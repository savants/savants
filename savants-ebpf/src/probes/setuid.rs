//! Privilege escalation probe — detects setuid/setgid calls.
//!
//! In eBPF mode: attaches to kprobe/__sys_setuid
//! In fallback mode: monitors /proc/pid/status for UID changes

/// Stub for fallback polling. Full eBPF implementation will attach to setuid syscall.
pub fn poll_proc(_hostname: &str) -> Option<Vec<crate::events::KernelSecurityEvent>> {
    // Privilege escalation detection requires real eBPF to catch in real-time.
    // The /proc fallback would only see the result (new UID) after the fact,
    // not the actual setuid() call.
    None
}
