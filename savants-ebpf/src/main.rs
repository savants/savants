//! Savants eBPF agent — kernel-level security and performance telemetry.
//!
//! Attaches eBPF probes to key kernel functions and emits events when
//! security-relevant activity is detected. Events are written to the
//! Savants graph as KernelSecurityEvent nodes.
//!
//! Probes:
//!   execve    — new process creation (detect reverse shells, cryptominers)
//!   connect   — outbound connections (detect C2 beacons, data exfiltration)
//!   openat    — file access on sensitive paths (detect secret theft)
//!   setuid    — privilege escalation (detect container escape)
//!
//! Requires: Linux 5.8+, CAP_BPF or root
//!
//! Usage:
//!   savants-ebpf                    # run standalone
//!   savants up --security           # started automatically by savants
//!   kubectl apply -f agent-ebpf.yaml  # K8s DaemonSet

use clap::Parser;
use colored::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

mod probes;
mod events;
mod graph_writer;

#[derive(Parser)]
#[command(name = "savants-ebpf")]
#[command(about = "Savants kernel-level security probe")]
struct Cli {
    /// Graph port to write events to
    #[arg(long, default_value = "6379")]
    port: u16,

    /// Graph name
    #[arg(long, default_value = "savants")]
    memory: String,

    /// Hostname override
    #[arg(long)]
    hostname: Option<String>,

    /// Only watch these namespaces (comma-separated, K8s pod filtering)
    #[arg(long)]
    namespaces: Option<String>,

    /// Disable specific probes
    #[arg(long)]
    disable_probe: Vec<String>,

    /// Dry run — print events to stdout instead of writing to graph
    #[arg(long)]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("savants_ebpf=info")
        .init();

    let cli = Cli::parse();
    let hostname = cli.hostname.unwrap_or_else(|| {
        gethostname::gethostname().to_string_lossy().to_string()
    });

    println!("{}", "Savants eBPF Security Probe".bold());
    println!("  Host: {}", hostname.cyan());
    println!("  Port: {}", cli.port);

    // Check kernel version
    let kernel = std::fs::read_to_string("/proc/version").unwrap_or_default();
    println!("  Kernel: {}", kernel.split_whitespace().nth(2).unwrap_or("unknown"));

    // Check capabilities
    if !check_capabilities() {
        eprintln!("{}: Insufficient privileges. Need CAP_BPF or root.", "Error".red());
        eprintln!("  Run with: sudo savants-ebpf");
        eprintln!("  Or in K8s: deploy as privileged DaemonSet");
        std::process::exit(1);
    }

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || r.store(false, Ordering::SeqCst))
        .expect("Failed to set Ctrl-C handler");

    // Initialize graph writer
    let writer = if cli.dry_run {
        println!("  Mode: {} (printing to stdout)", "dry-run".yellow());
        None
    } else {
        match graph_writer::GraphWriter::new(&hostname, cli.port, &cli.memory) {
            Ok(w) => {
                println!("  Mode: {} (writing to graph)", "live".green());
                Some(w)
            }
            Err(e) => {
                eprintln!("{}: Cannot connect to graph: {}", "Warning".yellow(), e);
                eprintln!("  Running in dry-run mode");
                None
            }
        }
    };

    println!();
    println!("Loading eBPF probes...");

    // Load probes
    let enabled_probes = probes::ProbeSet::load(&cli.disable_probe)?;
    println!("  Loaded {} probes: {}",
        enabled_probes.count().to_string().green(),
        enabled_probes.names().join(", "));

    println!();
    println!("{}", "Watching kernel events (Ctrl-C to stop)...".bold());

    // Event loop
    let mut event_count: u64 = 0;
    while running.load(Ordering::SeqCst) {
        // Poll for events from all probes
        if let Some(events) = enabled_probes.poll().await {
            for event in events {
                event_count += 1;

                // Apply filters
                if let Some(ref ns) = cli.namespaces {
                    if let Some(ref event_ns) = event.namespace {
                        if !ns.split(',').any(|n| n.trim() == event_ns) {
                            continue;
                        }
                    }
                }

                // Output
                if let Some(ref w) = writer {
                    w.write_event(&event);
                } else {
                    // Dry run: print to stdout
                    println!("{}", event.format());
                }
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    println!();
    println!("Stopped. {} kernel events captured.", event_count);
    Ok(())
}

fn check_capabilities() -> bool {
    // Check if we have CAP_BPF or are root
    if nix::unistd::geteuid().is_root() {
        return true;
    }
    // Check CAP_BPF via /proc/self/status
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if line.starts_with("CapEff:") {
                let hex = line.split_whitespace().nth(1).unwrap_or("0");
                if let Ok(caps) = u64::from_str_radix(hex.trim_start_matches("0x"), 16) {
                    let cap_bpf = 1u64 << 39; // CAP_BPF
                    return caps & cap_bpf != 0;
                }
            }
        }
    }
    false
}
