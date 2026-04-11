//! Bridge to the Python agent for heavy-lifting operations (ingestors, MCP server).
//!
//! The Rust CLI handles all user interaction and graph queries. When it needs
//! to run an ingestor or the MCP server, it spawns the Python agent as a
//! subprocess. The user never knows Python is involved.

use std::env;
use std::process::{Command, Stdio};

/// Find the Python interpreter that has the savants package installed.
fn find_python() -> String {
    // 1. Check for savants venv
    let home = dirs::home_dir().unwrap_or_default();
    let venv_python = home.join(".savants").join("venv").join("bin").join("python");
    if venv_python.exists() {
        return venv_python.to_string_lossy().to_string();
    }

    // 2. Check for project venv (development)
    if let Ok(cwd) = env::current_dir() {
        let dev_venv = cwd.join(".venv").join("bin").join("python");
        if dev_venv.exists() {
            return dev_venv.to_string_lossy().to_string();
        }
    }

    // 3. Check SAVANTS_PYTHON env var
    if let Ok(p) = env::var("SAVANTS_PYTHON") {
        return p;
    }

    // 4. Fallback to system python
    "python3".to_string()
}

fn python_env() -> Vec<(String, String)> {
    let mut env_vars = vec![];
    // Forward FALKORDB env vars
    for key in &["FALKORDB_HOST", "FALKORDB_PORT", "FALKORDB_GRAPH"] {
        if let Ok(val) = env::var(key) {
            env_vars.push((key.to_string(), val));
        }
    }
    // Add PYTHONPATH if in dev mode (src/ layout)
    if let Ok(cwd) = env::current_dir() {
        let src = cwd.join("src");
        if src.exists() {
            env_vars.push(("PYTHONPATH".to_string(), src.to_string_lossy().to_string()));
        }
    }
    env_vars
}

/// Run a savants Python CLI subcommand (e.g., `["k8s", "snapshot", "my-cluster"]`).
pub fn run_python(args: &[&str]) {
    let python = find_python();
    let mut cmd = Command::new(&python);
    cmd.arg("-m").arg("savants.cli");
    for arg in args {
        cmd.arg(arg);
    }
    for (k, v) in python_env() {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    match cmd.status() {
        Ok(status) => {
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
        }
        Err(e) => {
            eprintln!("Failed to run Python agent: {}", e);
            eprintln!("Make sure Python 3 and the savants package are installed.");
            eprintln!("Install with: pip install savants");
            std::process::exit(1);
        }
    }
}

/// Run a raw Python command (e.g., `["-m", "savants.mcp"]`).
pub fn run_python_raw(args: &[&str]) {
    let python = find_python();
    let mut cmd = Command::new(&python);
    for arg in args {
        cmd.arg(arg);
    }
    for (k, v) in python_env() {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    match cmd.status() {
        Ok(status) => {
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
        }
        Err(e) => {
            eprintln!("Failed to start MCP server: {}", e);
            std::process::exit(1);
        }
    }
}
