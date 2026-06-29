//! GitHub repository watcher — polls for new commits and re-indexes the code graph.
//!
//! Part of the daemon: monitors one or more GitHub repos for changes.
//! When a new commit is detected, it pulls the changes and re-runs
//! the code indexer to update the graph.
//!
//! Later: replaced by a GitHub App webhook for instant notification.

use std::process::Command;
use std::path::Path;

pub struct RepoWatcher {
    pub repo_path: String,
    pub remote: String,
    pub branch: String,
    last_sha: Option<String>,
}

impl RepoWatcher {
    pub fn new(repo_path: &str, branch: &str) -> Self {
        let last_sha = get_head_sha(repo_path);
        Self {
            repo_path: repo_path.to_string(),
            remote: "origin".to_string(),
            branch: branch.to_string(),
            last_sha,
        }
    }

    /// Check for new commits. Returns true if the code changed.
    pub fn poll(&mut self) -> bool {
        // Fetch from remote
        let fetch = Command::new("git")
            .args(["fetch", &self.remote, &self.branch, "--quiet"])
            .current_dir(&self.repo_path)
            .output();

        if fetch.is_err() { return false; }

        // Check if remote HEAD changed
        let remote_sha = get_remote_sha(&self.repo_path, &self.remote, &self.branch);

        if remote_sha.is_none() { return false; }
        if remote_sha == self.last_sha { return false; }

        // New commits! Pull them
        println!("[repo] New commits detected in {}", self.repo_path);
        println!("[repo] {} → {}",
            self.last_sha.as_deref().unwrap_or("none"),
            remote_sha.as_deref().unwrap_or("none"));

        let pull = Command::new("git")
            .args(["pull", "--ff-only", &self.remote, &self.branch])
            .current_dir(&self.repo_path)
            .output();

        if let Ok(output) = pull {
            if output.status.success() {
                // Show what changed
                if let (Some(old), Some(new)) = (&self.last_sha, &remote_sha) {
                    let diff = Command::new("git")
                        .args(["log", "--oneline", &format!("{}..{}", old, new)])
                        .current_dir(&self.repo_path)
                        .output();
                    if let Ok(d) = diff {
                        let commits = String::from_utf8_lossy(&d.stdout);
                        for line in commits.lines().take(5) {
                            println!("[repo]   {}", line);
                        }
                    }
                }

                self.last_sha = remote_sha;
                return true;
            }
        }

        false
    }

    /// Get the list of files that changed since the last known SHA.
    pub fn changed_files(&self) -> Vec<String> {
        if let Some(old_sha) = &self.last_sha {
            let output = Command::new("git")
                .args(["diff", "--name-only", old_sha, "HEAD"])
                .current_dir(&self.repo_path)
                .output();
            if let Ok(o) = output {
                return String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|l| l.to_string())
                    .collect();
            }
        }
        vec![]
    }
}

fn get_head_sha(repo_path: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn get_remote_sha(repo_path: &str, remote: &str, branch: &str) -> Option<String> {
    let ref_name = format!("{}/{}", remote, branch);
    let output = Command::new("git")
        .args(["rev-parse", &ref_name])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}
