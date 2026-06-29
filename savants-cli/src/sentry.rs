//! Sentry integration - pulls issues, stack traces, and events into the graph.
//!
//! Graph nodes: SentryIssue, SentryFrame
//! Edges: HAS_FRAME, CRASHES_IN (to CodeFunction), REPORTED_BY_SENTRY (from SlackMessage)

use crate::graph::GraphClient;
use std::collections::HashSet;

pub struct SentryIngestor {
    token: String,
    org: String,
    client: reqwest::blocking::Client,
}

impl SentryIngestor {
    pub fn new(token: String, org: String) -> Self {
        Self {
            token,
            org,
            client: reqwest::blocking::Client::new(),
        }
    }

    pub fn from_env() -> Option<Self> {
        let token = std::env::var("SAVANTS_SENTRY_TOKEN").ok()?;
        let org = std::env::var("SAVANTS_SENTRY_ORG").ok()?;
        Some(Self::new(token, org))
    }

    fn get(&self, path: &str) -> Option<serde_json::Value> {
        let url = format!("https://sentry.io/api/0{}", path);
        let resp = self.client.get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .timeout(std::time::Duration::from_secs(30))
            .send().ok()?;
        resp.json().ok()
    }

    pub fn ingest(&self, graph: &GraphClient, repo: &str) -> IngestStats {
        let mut stats = IngestStats::default();

        // Fetch unresolved issues
        let issues = match self.get(&format!("/organizations/{}/issues/?query=is:unresolved&limit=50", self.org)) {
            Some(serde_json::Value::Array(arr)) => arr,
            _ => return stats,
        };

        stats.issues = issues.len();

        for issue in &issues {
            let issue_id = issue.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let title = issue.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let count = issue.get("count").and_then(|v| v.as_str()).unwrap_or("0");
            let level = issue.get("level").and_then(|v| v.as_str()).unwrap_or("error");
            let project = issue.get("project").and_then(|p| p.get("slug")).and_then(|v| v.as_str()).unwrap_or("");
            let first_seen = issue.get("firstSeen").and_then(|v| v.as_str()).unwrap_or("");
            let last_seen = issue.get("lastSeen").and_then(|v| v.as_str()).unwrap_or("");
            let short_id = issue.get("shortId").and_then(|v| v.as_str()).unwrap_or("");

            if issue_id.is_empty() { continue; }

            let _ = graph.query(
                &format!(
                    "MERGE (s:SentryIssue {{id: '{}'}}) SET s.title = '{}', s.count = {}, s.level = '{}', \
                     s.project = '{}', s.first_seen = '{}', s.last_seen = '{}', s.short_id = '{}'",
                    esc(issue_id), esc(title), count, esc(level),
                    esc(project), esc(&first_seen[..std::cmp::min(19, first_seen.len())]),
                    esc(&last_seen[..std::cmp::min(19, last_seen.len())]), esc(short_id)
                ),
                &[],
            );

            // Get latest event with stack trace
            if let Some(event) = self.get(&format!("/organizations/{}/issues/{}/events/latest/", self.org, issue_id)) {
                if let Some(entries) = event.get("entries").and_then(|e| e.as_array()) {
                    for entry in entries {
                        if entry.get("type").and_then(|t| t.as_str()) != Some("exception") { continue; }
                        if let Some(values) = entry.get("data").and_then(|d| d.get("values")).and_then(|v| v.as_array()) {
                            for exc in values {
                                let exc_type = exc.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                let exc_value = exc.get("value").and_then(|v| v.as_str()).unwrap_or("");

                                let _ = graph.query(
                                    &format!(
                                        "MATCH (s:SentryIssue {{id: '{}'}}) SET s.exception_type = '{}', s.exception_value = '{}'",
                                        esc(issue_id), esc(exc_type), esc(&exc_value.chars().take(200).collect::<String>())
                                    ),
                                    &[],
                                );

                                let frames = exc.get("stacktrace")
                                    .or_else(|| Some(&serde_json::Value::Null))
                                    .and_then(|st| st.get("frames"))
                                    .and_then(|f| f.as_array());

                                if let Some(frames) = frames {
                                    for frame in frames {
                                        if !frame.get("inApp").and_then(|v| v.as_bool()).unwrap_or(false) { continue; }

                                        let func = frame.get("function").and_then(|v| v.as_str()).unwrap_or("");
                                        let filename = frame.get("filename").and_then(|v| v.as_str()).unwrap_or("");
                                        let lineno = frame.get("lineNo").and_then(|v| v.as_i64()).unwrap_or(0);

                                        if func.is_empty() { continue; }

                                        stats.frames += 1;

                                        let _ = graph.query(
                                            &format!(
                                                "MERGE (f:SentryFrame {{issue_id: '{}', function: '{}', line: {}}}) \
                                                 SET f.filename = '{}', f.in_app = true",
                                                esc(issue_id), esc(func), lineno, esc(filename)
                                            ),
                                            &[],
                                        );

                                        let _ = graph.query(
                                            &format!(
                                                "MATCH (s:SentryIssue {{id: '{}'}}), (f:SentryFrame {{issue_id: '{}', function: '{}', line: {}}}) \
                                                 MERGE (s)-[:HAS_FRAME]->(f)",
                                                esc(issue_id), esc(issue_id), esc(func), lineno
                                            ),
                                            &[],
                                        );

                                        // Link to CodeFunction if it exists
                                        if func.len() > 3 {
                                            let _ = graph.query(
                                                &format!(
                                                    "MATCH (si:SentryIssue {{id: '{}'}}), (cf:CodeFunction {{repo: '{}', name: '{}'}}) \
                                                     MERGE (si)-[:CRASHES_IN]->(cf)",
                                                    esc(issue_id), esc(repo), esc(func)
                                                ),
                                                &[],
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Cross-reference with Slack
            if title.len() > 10 {
                let title_search = &title[..std::cmp::min(50, title.len())];
                let _ = graph.query(
                    &format!(
                        "MATCH (si:SentryIssue {{id: '{}'}}), (m:SlackMessage) \
                         WHERE m.channel_name = 'prod-errors' AND toLower(m.text) CONTAINS toLower('{}') \
                         MERGE (m)-[:REPORTED_BY_SENTRY]->(si)",
                        esc(issue_id), esc(title_search)
                    ),
                    &[],
                );
            }
        }

        stats
    }
}

#[derive(Default)]
pub struct IngestStats {
    pub issues: usize,
    pub frames: usize,
}

impl IngestStats {
    pub fn summary(&self) -> String {
        format!("{} issues, {} stack frames", self.issues, self.frames)
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', " ").replace('\r', "")
}
