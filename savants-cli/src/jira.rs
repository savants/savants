//! Jira integration - pulls tickets, statuses, assignees into the graph.
//!
//! Graph nodes: JiraTicket
//! Edges: CHILD_OF, REFERENCES_TICKET (from SlackMessage), RESOLVES (from Commit/PR)

use crate::graph::GraphClient;
use std::io::Read;

pub struct JiraIngestor {
    url: String,
    user: String,
    token: String,
    project: String,
    client: reqwest::blocking::Client,
}

impl JiraIngestor {
    pub fn new(url: String, user: String, token: String, project: String) -> Self {
        Self { url, user, token, project, client: reqwest::blocking::Client::new() }
    }

    pub fn from_env() -> Option<Self> {
        let url = std::env::var("SAVANTS_JIRA_URL").ok()?;
        let user = std::env::var("SAVANTS_JIRA_USER").ok()?;
        let token = std::env::var("SAVANTS_JIRA_TOKEN").ok()?;
        let project = std::env::var("SAVANTS_JIRA_PROJECT").unwrap_or_else(|_| "VSCV".to_string());
        Some(Self::new(url, user, token, project))
    }

    fn post_json(&self, path: &str, body: &serde_json::Value) -> Option<serde_json::Value> {
        let url = format!("{}{}", self.url, path);
        let resp = self.client.post(&url)
            .basic_auth(&self.user, Some(&self.token))
            .header("Content-Type", "application/json")
            .json(body)
            .timeout(std::time::Duration::from_secs(30))
            .send().ok()?;
        resp.json().ok()
    }

    pub fn ingest(&self, graph: &GraphClient) -> IngestStats {
        let mut stats = IngestStats::default();

        // Fetch all projects to discover their keys
        let projects = self.discover_projects();

        for project_key in &projects {
            let mut start = 0;
            loop {
                let body = serde_json::json!({
                    "jql": format!("project = {} ORDER BY updated DESC", project_key),
                    "maxResults": 100,
                    "startAt": start,
                    "fields": ["key", "summary", "status", "assignee", "reporter",
                               "priority", "issuetype", "created", "updated",
                               "resolution", "labels", "parent"]
                });

                let data = match self.post_json("/rest/api/3/search/jql", &body) {
                    Some(d) => d,
                    None => break,
                };

                let issues = match data.get("issues").and_then(|i| i.as_array()) {
                    Some(arr) => arr.clone(),
                    None => break,
                };

                if issues.is_empty() { break; }

                for issue in &issues {
                    self.ingest_issue(graph, issue);
                    stats.tickets += 1;
                }

                if issues.len() < 100 { break; }
                start += issues.len();
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }

        // Cross-reference tickets with Slack, commits, PRs
        self.cross_reference(graph, &mut stats);

        stats
    }

    fn discover_projects(&self) -> Vec<String> {
        if !self.project.is_empty() {
            return self.project.split(',').map(|s| s.trim().to_string()).collect();
        }

        let url = format!("{}/rest/api/3/project", self.url);
        let resp = self.client.get(&url)
            .basic_auth(&self.user, Some(&self.token))
            .timeout(std::time::Duration::from_secs(15))
            .send();

        match resp {
            Ok(r) => {
                if let Ok(projects) = r.json::<Vec<serde_json::Value>>() {
                    projects.iter()
                        .filter_map(|p| p.get("key").and_then(|k| k.as_str()).map(|s| s.to_string()))
                        .collect()
                } else { vec![] }
            }
            Err(_) => vec![],
        }
    }

    fn ingest_issue(&self, graph: &GraphClient, issue: &serde_json::Value) {
        let key = issue.get("key").and_then(|v| v.as_str()).unwrap_or("");
        if key.is_empty() { return; }

        let f = match issue.get("fields") {
            Some(f) => f,
            None => return,
        };

        let summary = f.get("summary").and_then(|v| v.as_str()).unwrap_or("");
        let status = f.get("status").and_then(|s| s.get("name")).and_then(|v| v.as_str()).unwrap_or("");
        let status_cat = f.get("status").and_then(|s| s.get("statusCategory")).and_then(|c| c.get("name")).and_then(|v| v.as_str()).unwrap_or("");
        let assignee = f.get("assignee").and_then(|a| a.get("displayName")).and_then(|v| v.as_str()).unwrap_or("");
        let reporter = f.get("reporter").and_then(|r| r.get("displayName")).and_then(|v| v.as_str()).unwrap_or("");
        let priority = f.get("priority").and_then(|p| p.get("name")).and_then(|v| v.as_str()).unwrap_or("");
        let itype = f.get("issuetype").and_then(|t| t.get("name")).and_then(|v| v.as_str()).unwrap_or("");
        let created = f.get("created").and_then(|v| v.as_str()).unwrap_or("");
        let updated = f.get("updated").and_then(|v| v.as_str()).unwrap_or("");
        let resolution = f.get("resolution").and_then(|r| r.get("name")).and_then(|v| v.as_str()).unwrap_or("");
        let parent_key = f.get("parent").and_then(|p| p.get("key")).and_then(|v| v.as_str()).unwrap_or("");

        let _ = graph.query(
            &format!(
                "MERGE (t:JiraTicket {{key: '{}'}}) SET t.summary = '{}', t.status = '{}', \
                 t.status_category = '{}', t.assignee = '{}', t.reporter = '{}', \
                 t.priority = '{}', t.type = '{}', t.created = '{}', t.updated = '{}', \
                 t.resolution = '{}'",
                esc(key), esc(summary), esc(status), esc(status_cat), esc(assignee),
                esc(reporter), esc(priority), esc(itype), esc(created), esc(updated), esc(resolution)
            ),
            &[],
        );

        if !parent_key.is_empty() {
            let _ = graph.query(
                &format!(
                    "MATCH (t:JiraTicket {{key: '{}'}}), (p:JiraTicket {{key: '{}'}}) MERGE (t)-[:CHILD_OF]->(p)",
                    esc(key), esc(parent_key)
                ),
                &[],
            );
        }
    }

    fn cross_reference(&self, graph: &GraphClient, stats: &mut IngestStats) {
        // Link Slack messages mentioning ticket keys
        let _ = graph.query(
            "MATCH (t:JiraTicket), (m:SlackMessage) WHERE m.text CONTAINS t.key MERGE (m)-[:REFERENCES_TICKET]->(t)",
            &[],
        );

        // Link commits that reference ticket keys
        let _ = graph.query(
            "MATCH (t:JiraTicket), (c:Commit) WHERE c.message CONTAINS t.key MERGE (c)-[:RESOLVES]->(t)",
            &[],
        );

        // Link PRs by branch name or title
        let _ = graph.query(
            "MATCH (t:JiraTicket), (p:GitHubPR) WHERE p.branch CONTAINS t.key OR p.title CONTAINS t.key MERGE (p)-[:RESOLVES]->(t)",
            &[],
        );
    }
}

#[derive(Default)]
pub struct IngestStats {
    pub tickets: usize,
}

impl IngestStats {
    pub fn summary(&self) -> String {
        format!("{} tickets", self.tickets)
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', " ").replace('\r', "")
}
