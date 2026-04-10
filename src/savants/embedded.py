"""Embedded FalkorDB graph manager for the Savants CLI.

Manages a local FalkorDB instance (redis-server + falkordb.so module)
as a background subprocess. This eliminates the need for an external
FalkorDB server, port-forwards, or Docker — the graph "just works"
as part of the Savants install.

The bundled binaries live in `src/savants/binaries/` (symlinked from
the desktop Tauri app). On first run, `savants init` calls `start()`
which spawns redis-server with the FalkorDB module on a local port
(default 16379 to avoid colliding with any existing Redis on 6379).

Data is persisted to `~/.savants/data/` via RDB snapshots so the
graph survives process restarts. A PID file at `~/.savants/savants.pid`
tracks the running instance.

Usage:

    from savants.embedded import EmbeddedGraph
    eg = EmbeddedGraph()
    eg.start()      # spawns redis+falkordb, blocks until ready
    eg.client()     # returns a connected GraphClient
    eg.stop()       # graceful shutdown
"""

from __future__ import annotations

import logging
import os
import signal
import socket
import subprocess
import time
from pathlib import Path

logger = logging.getLogger(__name__)

SAVANTS_HOME = Path.home() / ".savants"
DATA_DIR = SAVANTS_HOME / "data"
PID_FILE = SAVANTS_HOME / "savants.pid"
LOG_FILE = SAVANTS_HOME / "falkordb.log"
DEFAULT_PORT = 16379


def _find_binaries() -> tuple[Path, Path]:
    """Locate the bundled redis-server and falkordb.so module.

    Searches in order:
    1. Adjacent to this file (installed package / development)
    2. ~/.savants/bin/ (standalone binary install via curl savants.sh)
    """
    candidates = [
        Path(__file__).parent / "binaries",
        SAVANTS_HOME / "bin",
    ]
    for d in candidates:
        redis = d / "redis-server-bundled"
        falkor = d / "falkordb.so"
        if redis.exists() and falkor.exists():
            return redis, falkor

    raise FileNotFoundError(
        "Could not find bundled redis-server and falkordb.so. "
        "Expected in src/savants/binaries/ or ~/.savants/bin/. "
        "Re-install Savants or run `savants init --download` to fetch them."
    )


def _port_in_use(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        return s.connect_ex(("127.0.0.1", port)) == 0


def _read_pid() -> int | None:
    try:
        pid = int(PID_FILE.read_text().strip())
        # Check if process is still alive
        os.kill(pid, 0)
        return pid
    except (FileNotFoundError, ValueError, ProcessLookupError, PermissionError):
        return None


class EmbeddedGraph:
    """Manages a local FalkorDB subprocess."""

    def __init__(self, port: int = DEFAULT_PORT):
        self.port = port
        self._proc: subprocess.Popen | None = None

    def start(self, timeout: float = 10.0) -> None:
        """Start the embedded FalkorDB if not already running.

        Idempotent: if a healthy instance is already on `self.port`,
        this is a no-op.
        """
        # Check if already running (from a previous savants invocation)
        existing_pid = _read_pid()
        if existing_pid is not None and _port_in_use(self.port):
            logger.info("Embedded FalkorDB already running (pid=%d, port=%d)",
                       existing_pid, self.port)
            return

        # If port is in use but not ours, don't clobber it
        if _port_in_use(self.port):
            logger.info("Port %d already in use (external FalkorDB?), reusing",
                       self.port)
            return

        # Ensure data directory exists
        DATA_DIR.mkdir(parents=True, exist_ok=True)
        SAVANTS_HOME.mkdir(parents=True, exist_ok=True)

        redis_bin, falkordb_so = _find_binaries()

        # Resolve symlinks so subprocess gets real paths
        redis_bin = redis_bin.resolve()
        falkordb_so = falkordb_so.resolve()

        # Find libgomp for FalkorDB
        env = os.environ.copy()
        ld_path = env.get("LD_LIBRARY_PATH", "")
        # Common locations for libgomp
        for search in ["/usr/lib", "/usr/local/lib", "/nix/store"]:
            try:
                result = subprocess.run(
                    ["find", search, "-maxdepth", "4", "-name", "libgomp.so.1"],
                    capture_output=True, text=True, timeout=3,
                )
                if result.stdout.strip():
                    gomp_dir = str(Path(result.stdout.strip().splitlines()[0]).parent)
                    ld_path = f"{gomp_dir}:{ld_path}" if ld_path else gomp_dir
                    break
            except Exception:
                continue
        env["LD_LIBRARY_PATH"] = ld_path

        log_fh = open(LOG_FILE, "a")
        self._proc = subprocess.Popen(
            [
                str(redis_bin),
                "--port", str(self.port),
                "--daemonize", "no",
                "--dir", str(DATA_DIR),
                "--dbfilename", "savants.rdb",
                "--save", "60", "1",  # RDB snapshot every 60s if ≥1 change
                "--loadmodule", str(falkordb_so),
                "--loglevel", "warning",
            ],
            env=env,
            stdout=log_fh,
            stderr=log_fh,
            start_new_session=True,  # survives parent exit
        )

        # Write PID file
        PID_FILE.write_text(str(self._proc.pid))

        # Wait for the port to become available
        t0 = time.time()
        while time.time() - t0 < timeout:
            if _port_in_use(self.port):
                logger.info("Embedded FalkorDB started (pid=%d, port=%d)",
                           self._proc.pid, self.port)
                return
            # Check if process died
            if self._proc.poll() is not None:
                raise RuntimeError(
                    f"FalkorDB exited with code {self._proc.returncode}. "
                    f"Check {LOG_FILE} for details."
                )
            time.sleep(0.1)

        raise TimeoutError(
            f"FalkorDB did not start within {timeout}s. Check {LOG_FILE}"
        )

    def stop(self) -> None:
        """Gracefully stop the embedded FalkorDB."""
        pid = _read_pid()
        if pid is not None:
            try:
                os.kill(pid, signal.SIGTERM)
                # Wait up to 5s for graceful shutdown
                for _ in range(50):
                    try:
                        os.kill(pid, 0)
                        time.sleep(0.1)
                    except ProcessLookupError:
                        break
            except ProcessLookupError:
                pass
        if self._proc is not None:
            try:
                self._proc.terminate()
                self._proc.wait(timeout=5)
            except Exception:
                pass
            self._proc = None
        try:
            PID_FILE.unlink(missing_ok=True)
        except Exception:
            pass
        logger.info("Embedded FalkorDB stopped")

    def is_running(self) -> bool:
        return _read_pid() is not None and _port_in_use(self.port)

    def client(self, graph_name: str = "savants") -> "GraphClient":
        """Return a GraphClient connected to the embedded instance."""
        from savants.config import FalkorDBConfig
        from savants.graph.client import GraphClient

        return GraphClient(FalkorDBConfig(
            host="localhost",
            port=self.port,
            graph_name=graph_name,
        ))

    def status(self) -> dict:
        """Return status info for the embedded graph."""
        pid = _read_pid()
        return {
            "running": self.is_running(),
            "pid": pid,
            "port": self.port,
            "data_dir": str(DATA_DIR),
            "log_file": str(LOG_FILE),
        }
