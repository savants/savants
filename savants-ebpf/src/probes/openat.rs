//! File access probe — detects access to sensitive files.
//!
//! In eBPF mode: attaches to tracepoint/syscalls/sys_enter_openat
//! In fallback mode: periodic check of /proc/pid/fd for sensitive paths

/// Stub for fallback polling. Full eBPF implementation will attach to openat syscall.
pub fn poll_proc(_hostname: &str) -> Option<Vec<crate::events::KernelSecurityEvent>> {
    // The openat probe is most effective with real eBPF — the /proc fallback
    // can't observe file opens in real-time. We'd need inotify for that,
    // which has scalability issues.
    //
    // For now, this probe only fires in eBPF mode.
    None
}
