// SynapCode Desktop — Tauri app managing FalkorDB, Temporal, and Python worker
//
// On launch:
//   1. Start FalkorDB (redis-server with falkordb module) as a child process
//   2. Wait for FalkorDB to be healthy (PING)
//   3. Start Temporal dev server as a child process
//   4. Start the Python Temporal worker as a child process
//   5. Expose health status to the UI via Tauri commands
//
// On quit: gracefully shut down all child processes in reverse order.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

/// Holds references to all managed child processes.
struct ManagedProcesses {
    falkordb: Mutex<Option<Child>>,
    temporal: Mutex<Option<Child>>,
    worker: Mutex<Option<Child>>,
}

#[derive(Serialize, Clone)]
struct ServiceStatus {
    name: String,
    running: bool,
    pid: Option<u32>,
    detail: String,
}

#[derive(Serialize, Clone)]
struct SystemHealth {
    services: Vec<ServiceStatus>,
    ram_used_gb: f64,
    ram_total_gb: f64,
    ram_percent: f64,
}

/// Resolve the path to a bundled sidecar binary.
fn sidecar_path(app: &AppHandle, name: &str) -> PathBuf {
    app.path()
        .resource_dir()
        .unwrap_or_default()
        .join("binaries")
        .join(name)
}

/// Check if FalkorDB is reachable at localhost:6379.
fn falkordb_is_healthy() -> bool {
    std::net::TcpStream::connect_timeout(
        &"127.0.0.1:6379".parse().unwrap(),
        Duration::from_secs(1),
    )
    .is_ok()
}

/// Check if Temporal is reachable at localhost:7233.
fn temporal_is_healthy() -> bool {
    std::net::TcpStream::connect_timeout(
        &"127.0.0.1:7233".parse().unwrap(),
        Duration::from_secs(1),
    )
    .is_ok()
}

/// Start FalkorDB (redis-server with falkordb module).
fn start_falkordb(app: &AppHandle) -> Result<Child, String> {
    let bin = sidecar_path(app, "falkordb-server");

    // Try bundled binary first, fall back to system redis-server
    let (cmd, args): (String, Vec<String>) = if bin.exists() {
        (bin.to_string_lossy().into(), vec![])
    } else {
        // Fall back: assume redis-server is on PATH with FalkorDB module
        (
            "redis-server".into(),
            vec![
                "--port".into(),
                "6379".into(),
                "--loadmodule".into(),
                find_falkordb_module().unwrap_or_else(|| "/usr/lib/redis/modules/falkordb.so".into()),
                "--save".into(),
                "60".into(),
                "1".into(),
                "--daemonize".into(),
                "no".into(),
            ],
        )
    };

    tracing::info!("Starting FalkorDB: {} {:?}", cmd, args);
    Command::new(&cmd)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start FalkorDB: {e}"))
}

/// Locate the FalkorDB shared library module.
fn find_falkordb_module() -> Option<String> {
    let candidates = [
        "/usr/lib/redis/modules/falkordb.so",
        "/usr/local/lib/redis/modules/falkordb.so",
        "/opt/homebrew/lib/redis/modules/falkordb.so",
    ];
    candidates.iter().find(|p| std::path::Path::new(p).exists()).map(|s| s.to_string())
}

/// Start Temporal dev server.
fn start_temporal(app: &AppHandle) -> Result<Child, String> {
    let bin = sidecar_path(app, "temporal-server");

    let (cmd, args): (String, Vec<String>) = if bin.exists() {
        (bin.to_string_lossy().into(), vec!["server".into(), "start-dev".into()])
    } else {
        // Fall back to system temporal CLI
        ("temporal".into(), vec!["server".into(), "start-dev".into(), "--headless".into()])
    };

    tracing::info!("Starting Temporal: {} {:?}", cmd, args);
    Command::new(&cmd)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start Temporal: {e}"))
}

/// Start the Python Temporal worker.
fn start_python_worker() -> Result<Child, String> {
    tracing::info!("Starting Python worker");
    Command::new("python")
        .args(["-m", "synapcode.temporal.worker"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start Python worker: {e}"))
}

/// Wait for a service to become healthy, with timeout.
fn wait_for_health(check: fn() -> bool, name: &str, timeout_secs: u64) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    while std::time::Instant::now() < deadline {
        if check() {
            tracing::info!("{} is healthy", name);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!("{} did not become healthy within {}s", name, timeout_secs))
}

fn is_process_running(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

// --- Tauri Commands (exposed to the UI) ---

#[tauri::command]
fn get_health(procs: State<ManagedProcesses>) -> SystemHealth {
    let mut services = vec![];

    // FalkorDB
    let falkordb_running = {
        let mut guard = procs.falkordb.lock().unwrap();
        guard.as_mut().map_or(false, |c| is_process_running(c))
    };
    services.push(ServiceStatus {
        name: "FalkorDB".into(),
        running: falkordb_running && falkordb_is_healthy(),
        pid: procs.falkordb.lock().unwrap().as_ref().map(|c| c.id()),
        detail: if falkordb_is_healthy() { "localhost:6379".into() } else { "unreachable".into() },
    });

    // Temporal
    let temporal_running = {
        let mut guard = procs.temporal.lock().unwrap();
        guard.as_mut().map_or(false, |c| is_process_running(c))
    };
    services.push(ServiceStatus {
        name: "Temporal".into(),
        running: temporal_running && temporal_is_healthy(),
        pid: procs.temporal.lock().unwrap().as_ref().map(|c| c.id()),
        detail: if temporal_is_healthy() { "localhost:7233".into() } else { "unreachable".into() },
    });

    // Python Worker
    let worker_running = {
        let mut guard = procs.worker.lock().unwrap();
        guard.as_mut().map_or(false, |c| is_process_running(c))
    };
    services.push(ServiceStatus {
        name: "Worker".into(),
        running: worker_running,
        pid: procs.worker.lock().unwrap().as_ref().map(|c| c.id()),
        detail: if worker_running { "synapcode-tasks queue".into() } else { "stopped".into() },
    });

    // System RAM
    let sys = sysinfo::System::new_all();
    let total = sys.total_memory() as f64 / 1_073_741_824.0;
    let used = sys.used_memory() as f64 / 1_073_741_824.0;

    SystemHealth {
        services,
        ram_used_gb: used,
        ram_total_gb: total,
        ram_percent: if total > 0.0 { (used / total) * 100.0 } else { 0.0 },
    }
}

#[tauri::command]
fn restart_service(name: String, app: AppHandle, procs: State<ManagedProcesses>) -> Result<String, String> {
    match name.as_str() {
        "falkordb" => {
            let mut guard = procs.falkordb.lock().unwrap();
            if let Some(ref mut child) = *guard {
                let _ = child.kill();
            }
            *guard = Some(start_falkordb(&app)?);
            Ok("FalkorDB restarted".into())
        }
        "temporal" => {
            let mut guard = procs.temporal.lock().unwrap();
            if let Some(ref mut child) = *guard {
                let _ = child.kill();
            }
            *guard = Some(start_temporal(&app)?);
            Ok("Temporal restarted".into())
        }
        "worker" => {
            let mut guard = procs.worker.lock().unwrap();
            if let Some(ref mut child) = *guard {
                let _ = child.kill();
            }
            *guard = Some(start_python_worker()?);
            Ok("Worker restarted".into())
        }
        _ => Err(format!("Unknown service: {name}")),
    }
}

fn main() {
    tracing_subscriber::fmt::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(ManagedProcesses {
            falkordb: Mutex::new(None),
            temporal: Mutex::new(None),
            worker: Mutex::new(None),
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let procs = app.state::<ManagedProcesses>();

            // --- Boot sequence: start all services ---

            // 1. FalkorDB
            if !falkordb_is_healthy() {
                match start_falkordb(&handle) {
                    Ok(child) => {
                        *procs.falkordb.lock().unwrap() = Some(child);
                        if let Err(e) = wait_for_health(falkordb_is_healthy, "FalkorDB", 15) {
                            tracing::error!("{}", e);
                        }
                    }
                    Err(e) => tracing::error!("FalkorDB start failed: {}", e),
                }
            } else {
                tracing::info!("FalkorDB already running");
            }

            // 2. Temporal
            if !temporal_is_healthy() {
                match start_temporal(&handle) {
                    Ok(child) => {
                        *procs.temporal.lock().unwrap() = Some(child);
                        if let Err(e) = wait_for_health(temporal_is_healthy, "Temporal", 30) {
                            tracing::error!("{}", e);
                        }
                    }
                    Err(e) => tracing::error!("Temporal start failed: {}", e),
                }
            } else {
                tracing::info!("Temporal already running");
            }

            // 3. Python Worker (depends on both services)
            if falkordb_is_healthy() && temporal_is_healthy() {
                match start_python_worker() {
                    Ok(child) => {
                        *procs.worker.lock().unwrap() = Some(child);
                        tracing::info!("Python worker started");
                    }
                    Err(e) => tracing::error!("Worker start failed: {}", e),
                }
            }

            Ok(())
        })
        .on_event(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let procs = app.state::<ManagedProcesses>();
                // Graceful shutdown in reverse order
                if let Some(ref mut child) = *procs.worker.lock().unwrap() {
                    tracing::info!("Stopping Python worker");
                    let _ = child.kill();
                }
                if let Some(ref mut child) = *procs.temporal.lock().unwrap() {
                    tracing::info!("Stopping Temporal");
                    let _ = child.kill();
                }
                if let Some(ref mut child) = *procs.falkordb.lock().unwrap() {
                    tracing::info!("Stopping FalkorDB");
                    let _ = child.kill();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![get_health, restart_service])
        .run(tauri::generate_context!())
        .expect("error while running SynapCode");
}
