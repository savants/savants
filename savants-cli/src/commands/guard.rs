use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

/// Embedded guard-cli.sh script content.
/// This is kept in sync with scripts/guard-cli.sh in the repo.
const GUARD_SCRIPT: &str = include_str!("../../../scripts/guard-cli.sh");

fn guard_script_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".savants")
        .join("guard-cli.sh")
}

/// Ensure the embedded guard-cli.sh is written to ~/.savants/guard-cli.sh
/// and kept up to date with the version bundled in the binary.
fn ensure_script() -> PathBuf {
    let path = guard_script_path();

    // Check if existing script matches the embedded version
    let needs_write = match fs::read_to_string(&path) {
        Ok(existing) => existing != GUARD_SCRIPT,
        Err(_) => true,
    };

    if needs_write {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        if let Err(e) = fs::write(&path, GUARD_SCRIPT) {
            eprintln!("Warning: could not write guard script to {}: {}", path.display(), e);
            eprintln!("  Home dir: {:?}", dirs::home_dir());
            eprintln!("  Parent exists: {}", path.parent().map(|p| p.exists()).unwrap_or(false));
        } else {
            // Make executable (Unix only — on Windows, .exe extension determines executability)
            #[cfg(unix)]
            {
                if let Ok(meta) = fs::metadata(&path) {
                    let mut perms = meta.permissions();
                    perms.set_mode(0o755);
                    fs::set_permissions(&path, perms).ok();
                }
            }
        }
    }

    path
}

/// Embed all guard profile JSON files at compile time.
/// These are synced to ~/.savants/profiles/ on every run.
const PROFILES: &[(&str, &str)] = &[
    ("minimal.json", include_str!("../../../packages/guard-profiles/presets/minimal.json")),
    ("standard.json", include_str!("../../../packages/guard-profiles/presets/standard.json")),
    ("paranoid.json", include_str!("../../../packages/guard-profiles/presets/paranoid.json")),
    ("secrets.json", include_str!("../../../packages/guard-profiles/presets/secrets.json")),
    ("git-safe.json", include_str!("../../../packages/guard-profiles/presets/git-safe.json")),
    ("infra-safe.json", include_str!("../../../packages/guard-profiles/presets/infra-safe.json")),
    ("publish-safe.json", include_str!("../../../packages/guard-profiles/presets/publish-safe.json")),
    ("k8s-safe.json", include_str!("../../../packages/guard-profiles/presets/k8s-safe.json")),
    ("k8s-secrets.json", include_str!("../../../packages/guard-profiles/presets/k8s-secrets.json")),
    ("battle-tested.json", include_str!("../../../packages/guard-profiles/presets/battle-tested.json")),
    ("nixos-safe.json", include_str!("../../../packages/guard-profiles/presets/nixos-safe.json")),
];

/// Sync embedded profile files to ~/.savants/profiles/ if they've changed.
fn ensure_profiles() {
    let profiles_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".savants")
        .join("profiles");
    fs::create_dir_all(&profiles_dir).ok();

    for (name, content) in PROFILES {
        let path = profiles_dir.join(name);
        let needs_write = match fs::read_to_string(&path) {
            Ok(existing) => existing != *content,
            Err(_) => true,
        };
        if needs_write {
            fs::write(&path, content).ok();
        }
    }
}

pub fn run(args: Vec<String>) {
    let script = ensure_script();
    ensure_profiles();

    if !script.exists() {
        eprintln!("Error: guard script not found at {}", script.display());
        std::process::exit(1);
    }

    // Find bash — on Windows it may be at a non-standard path (Git Bash)
    let bash = if cfg!(windows) {
        // Try common Windows bash locations
        let candidates = [
            "bash".to_string(),
            "C:\\Program Files\\Git\\bin\\bash.exe".to_string(),
            "C:\\Program Files (x86)\\Git\\bin\\bash.exe".to_string(),
            format!("{}\\Git\\bin\\bash.exe", std::env::var("ProgramFiles").unwrap_or_default()),
        ];
        candidates.into_iter()
            .find(|p| Command::new(p).arg("--version").output().is_ok())
            .unwrap_or_else(|| "bash".to_string())
    } else {
        "bash".to_string()
    };

    let status = Command::new(&bash)
        .arg(&script)
        .args(&args)
        .env("HOME", dirs::home_dir().unwrap_or_default())
        .status();

    match status {
        Ok(s) => {
            if !s.success() {
                std::process::exit(s.code().unwrap_or(1));
            }
        }
        Err(e) => {
            eprintln!("Error: failed to run guard script with '{}': {}", bash, e);
            eprintln!("Script path: {}", script.display());
            if cfg!(windows) {
                eprintln!("Hint: Guard requires bash (Git Bash). Install Git for Windows: https://git-scm.com");
            }
            std::process::exit(1);
        }
    }
}
