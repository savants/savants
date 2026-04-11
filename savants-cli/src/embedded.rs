//! Embedded graph engine manager — starts the Savants memory backend as a subprocess.
//!
//! Eliminates the need for any external graph database. The user runs
//! `savants up` and the graph "just works."

use std::env;
use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use colored::*;

const DEFAULT_PORT: u16 = 6379;

pub struct EmbeddedFalkorDB {
    pub port: u16,
}

impl EmbeddedFalkorDB {
    pub fn new() -> Self {
        let port = env::var("SAVANTS_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_PORT);
        Self { port }
    }

    /// Ensure FalkorDB is running. Starts it if not already up.
    /// Returns true if we started it, false if it was already running.
    pub fn ensure_running(&self) -> Result<bool, String> {
        // Already running?
        if self.is_running() {
            return Ok(false);
        }

        // Find binaries
        let redis_bin = find_redis_binary()
            .ok_or("Could not find redis-server. Install Redis or run the savants installer.")?;
        let graph_module = find_graph_module();

        // Ensure data directory
        let data_dir = savants_home().join("data");
        fs::create_dir_all(&data_dir).map_err(|e| format!("mkdir failed: {}", e))?;

        // Build command
        let mut cmd = Command::new(&redis_bin);
        cmd.arg("--port").arg(self.port.to_string())
            .arg("--daemonize").arg("no")
            .arg("--dir").arg(data_dir.to_str().unwrap())
            .arg("--dbfilename").arg("savants.rdb")
            .arg("--save").arg("60").arg("1")
            .arg("--loglevel").arg("warning");

        if let Some(ref module) = graph_module {
            cmd.arg("--loadmodule").arg(module);
        }

        // Set LD_LIBRARY_PATH for libgomp (needed by FalkorDB on NixOS)
        let mut env_vars = env::vars().collect::<Vec<_>>();
        if let Some(gomp_dir) = find_libgomp() {
            let existing = env::var("LD_LIBRARY_PATH").unwrap_or_default();
            let new_path = if existing.is_empty() {
                gomp_dir
            } else {
                format!("{}:{}", gomp_dir, existing)
            };
            env_vars.retain(|(k, _)| k != "LD_LIBRARY_PATH");
            env_vars.push(("LD_LIBRARY_PATH".to_string(), new_path));
        }

        let log_file = savants_home().join("savants-engine.log");
        let log = fs::File::create(&log_file).map_err(|e| format!("log create: {}", e))?;

        let child = cmd
            .envs(env_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .stdin(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start redis-server: {}", e))?;

        // Write PID and port so other commands can find the running instance
        let pid_file = savants_home().join("savants.pid");
        let _ = fs::write(&pid_file, child.id().to_string());
        let port_file = savants_home().join("savants.port");
        let _ = fs::write(&port_file, self.port.to_string());

        // Wait for it to be ready
        let start = Instant::now();
        let timeout = Duration::from_secs(10);
        while start.elapsed() < timeout {
            if self.is_running() {
                return Ok(true);
            }
            thread::sleep(Duration::from_millis(100));
        }

        Err(format!(
            "FalkorDB did not start within 10s. Check {}",
            log_file.display()
        ))
    }

    pub fn is_running(&self) -> bool {
        TcpStream::connect(format!("127.0.0.1:{}", self.port)).is_ok()
    }
}

fn savants_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".savants")
}

fn find_redis_binary() -> Option<String> {
    // 1. Env override
    if let Ok(p) = env::var("REDIS_SERVER") {
        if Path::new(&p).exists() {
            return Some(p);
        }
    }

    // 2. Bundled binary (skip on NixOS — dynamic linking issues)
    if !is_nixos() {
        if let Some(bundled) = find_bundled("redis-server-bundled") {
            return Some(bundled);
        }
    }

    // 3. System PATH
    crate::find_in_path("redis-server").map(|p| p.to_string_lossy().to_string())
}

fn find_graph_module() -> Option<String> {
    // 1. Env override
    if let Ok(p) = env::var("SAVANTS_MODULE") {
        if Path::new(&p).exists() {
            return Some(p);
        }
    }

    // 2. Bundled
    if let Some(bundled) = find_bundled("falkordb.so") {
        return Some(bundled);
    }

    // 3. System paths
    for p in &[
        "/usr/lib/redis/modules/falkordb.so",
        "/usr/local/lib/redis/modules/falkordb.so",
    ] {
        if Path::new(p).exists() {
            return Some(p.to_string());
        }
    }

    None
}

fn find_bundled(filename: &str) -> Option<String> {
    // Check relative to executable
    if let Ok(exe) = env::current_exe() {
        let dir = exe.parent()?;
        // Same directory as binary
        let candidate = dir.join(filename);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
        // ../binaries/ (development layout)
        let candidate = dir.join("../binaries").join(filename);
        if candidate.exists() {
            return Some(candidate.canonicalize().ok()?.to_string_lossy().to_string());
        }
    }

    // Check in savants home
    let candidate = savants_home().join("bin").join(filename);
    if candidate.exists() {
        return Some(candidate.to_string_lossy().to_string());
    }

    // Walk up from cwd looking for desktop/src-tauri/binaries (dev checkout)
    if let Ok(cwd) = env::current_dir() {
        let mut dir = cwd.as_path();
        loop {
            let candidate = dir.join("desktop/src-tauri/binaries").join(filename);
            if candidate.exists() {
                return Some(candidate.to_string_lossy().to_string());
            }
            match dir.parent() {
                Some(parent) if parent != dir => dir = parent,
                _ => break,
            }
        }
    }

    None
}

fn find_libgomp() -> Option<String> {
    // NixOS-specific: find libgomp.so.1 in the nix store
    let output = Command::new("find")
        .args(["/nix/store", "-maxdepth", "4", "-name", "libgomp.so.1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_path = stdout.lines().next()?;
    Path::new(first_path).parent().map(|p| p.to_string_lossy().to_string())
}

fn is_nixos() -> bool {
    Path::new("/etc/NIXOS").exists() || Path::new("/run/current-system/sw").exists()
}
