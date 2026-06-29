//! GitHub integration - pulls PRs, reviews, CI status into the graph.
//!
//! Graph nodes: GitHubPR
//! Edges: RESOLVES (to JiraTicket), PART_OF_PR (from Commit)

use crate::graph::GraphClient;
use std::process::Command;

pub struct GitHubIngestor {
    repo: String,  // "owner/repo" format
    token: Option<String>,
}

impl GitHubIngestor {
    pub fn new(repo: String) -> Self {
        let token = std::env::var("GITHUB_TOKEN").ok()
            .or_else(|| std::env::var("GH_TOKEN").ok());
        Self { repo, token }
    }

    pub fn from_env() -> Option<Self> {
        let repo = std::env::var("SAVANTS_GITHUB_REPO").ok()?;
        Some(Self::new(repo))
    }

    pub fn ingest(&self, graph: &GraphClient) -> IngestStats {
        let mut stats = IngestStats::default();

        // Use gh CLI which handles auth automatically
        let output = Command::new("gh")
            .args([
                "pr", "list", "--repo", &self.repo,
                "--state", "all", "--limit", "200",
                "--json", "number,title,state,author,createdAt,closedAt,mergedAt,headRefName,url",
            ])
            .output();

        let prs: Vec<serde_json::Value> = match output {
            Ok(o) if o.status.success() => {
                serde_json::from_slice(&o.stdout).unwrap_or_default()
            }
            _ => return stats,
        };

        let repo_name = self.repo.split('/').last().unwrap_or(&self.repo);

        for pr in &prs {
            let number = pr.get("number").and_then(|v| v.as_i64()).unwrap_or(0);
            let title = pr.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let state = pr.get("state").and_then(|v| v.as_str()).unwrap_or("");
            let author = pr.get("author").and_then(|a| a.get("login")).and_then(|v| v.as_str()).unwrap_or("");
            let created = pr.get("createdAt").and_then(|v| v.as_str()).unwrap_or("");
            let merged = pr.get("mergedAt").and_then(|v| v.as_str()).unwrap_or("");
            let closed = pr.get("closedAt").and_then(|v| v.as_str()).unwrap_or("");
            let branch = pr.get("headRefName").and_then(|v| v.as_str()).unwrap_or("");
            let url = pr.get("url").and_then(|v| v.as_str()).unwrap_or("");

            if number == 0 { continue; }

            let _ = graph.query(
                &format!(
                    "MERGE (p:GitHubPR {{number: {}, repo: '{}'}}) \
                     SET p.title = '{}', p.state = '{}', p.author = '{}', \
                     p.created = '{}', p.merged = '{}', p.closed = '{}', \
                     p.branch = '{}', p.url = '{}'",
                    number, esc(repo_name), esc(title), esc(state), esc(author),
                    esc(created), esc(merged), esc(closed), esc(branch), esc(url)
                ),
                &[],
            );

            stats.prs += 1;

            // Link PR to commits by PR number in commit message
            let _ = graph.query(
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}}), (c:Commit {{repo: '{}'}}) \
                     WHERE c.message CONTAINS '#{}'  \
                     MERGE (c)-[:PART_OF_PR]->(p)",
                    number, esc(repo_name), esc(repo_name), number
                ),
                &[],
            );
        }

        // Cross-reference PRs with Jira tickets
        let _ = graph.query(
            &format!(
                "MATCH (p:GitHubPR {{repo: '{}'}}), (t:JiraTicket) \
                 WHERE p.branch CONTAINS t.key OR p.title CONTAINS t.key \
                 MERGE (p)-[:RESOLVES]->(t)",
                esc(repo_name)
            ),
            &[],
        );

        stats
    }
}

#[derive(Default)]
pub struct IngestStats {
    pub prs: usize,
}

impl IngestStats {
    pub fn summary(&self) -> String {
        format!("{} PRs", self.prs)
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', " ").replace('\r', "")
}
