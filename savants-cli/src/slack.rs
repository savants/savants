//! Slack graph ingestion — reads channel history, builds a deep graph of
//! conversations correlated with infrastructure events.
//!
//! Default mode: OBSERVE ONLY. Never posts to Slack unless explicitly configured.
//!
//! Graph nodes: SlackUser, SlackChannel, SlackMessage, SlackThread, SlackIncident
//! Cross-layer edges: MENTIONS_SERVICE, REPORTS_SYMPTOM, DURING_INCIDENT, etc.

use crate::graph::GraphClient;
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

/// Known symptom keywords that indicate infrastructure issues.
const SYMPTOM_KEYWORDS: &[&str] = &[
    "502", "503", "500", "timeout", "slow", "down", "broken", "crash",
    "oom", "oomkill", "oomkilled", "restart", "crashloop", "unhealthy",
    "degraded", "latency", "error rate", "connection refused", "unreachable",
    "disk full", "memory", "cpu spike", "can't connect", "not responding",
];

/// Keywords that indicate a resolution.
const RESOLUTION_KEYWORDS: &[&str] = &[
    "fixed", "resolved", "deployed", "rolled back", "rollback", "reverted",
    "merged", "patched", "restarted", "recovered", "back to normal",
    "all clear", "issue resolved", "root cause",
];

/// Keywords that indicate a command was run.
const COMMAND_PATTERNS: &[&str] = &[
    "kubectl", "helm", "terraform", "docker", "k9s", "argocd",
    "aws ", "gcloud", "az ", "flux", "kustomize",
];

pub struct SlackIngestor {
    token: String,
    cookie: Option<String>,
    client: reqwest::blocking::Client,
}

impl SlackIngestor {
    pub fn new(token: String, cookie: Option<String>) -> Self {
        Self {
            token,
            cookie,
            client: reqwest::blocking::Client::new(),
        }
    }

    pub fn from_env() -> Option<Self> {
        let token = std::env::var("SAVANTS_SLACK_USER_TOKEN")
            .or_else(|_| std::env::var("SAVANTS_SLACK_BOT_TOKEN"))
            .ok()?;
        let cookie = std::env::var("SAVANTS_SLACK_COOKIE").ok();
        Some(Self::new(token, cookie))
    }

    fn slack_get(&self, url: &str, params: &[(&str, &str)]) -> Option<serde_json::Value> {
        let mut req = self.client.get(url)
            .query(params)
            .header("Authorization", format!("Bearer {}", self.token))
            .timeout(std::time::Duration::from_secs(15));
        if let Some(ref c) = self.cookie {
            req = req.header("Cookie", format!("d={}", c));
        }
        let resp = req.send().ok()?;
        let data: serde_json::Value = resp.json().ok()?;
        if data.get("ok")?.as_bool()? {
            Some(data)
        } else {
            let err = data.get("error").and_then(|e| e.as_str()).unwrap_or("unknown");
            eprintln!("[slack] API error: {}", err);
            None
        }
    }

    /// Fetch all channels the user is a member of.
    pub fn list_channels(&self) -> Vec<(String, String, u64)> {
        let mut channels = vec![];
        let mut cursor = String::new();

        loop {
            let mut params = vec![
                ("types", "public_channel,private_channel"),
                ("exclude_archived", "true"),
                ("limit", "200"),
            ];
            if !cursor.is_empty() {
                params.push(("cursor", &cursor));
            }

            let data = match self.slack_get("https://slack.com/api/conversations.list", &params) {
                Some(d) => d,
                None => break,
            };

            if let Some(ch_list) = data.get("channels").and_then(|c| c.as_array()) {
                for ch in ch_list {
                    let id = ch.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let members = ch.get("num_members").and_then(|v| v.as_u64()).unwrap_or(0);
                    let is_member = ch.get("is_member").and_then(|v| v.as_bool()).unwrap_or(false);
                    if is_member && !id.is_empty() {
                        channels.push((id.to_string(), name.to_string(), members));
                    }
                }
            }

            // Pagination
            cursor = data.get("response_metadata")
                .and_then(|m| m.get("next_cursor"))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            if cursor.is_empty() { break; }
        }

        channels
    }

    /// Fetch message history for a channel.
    /// `oldest` is a Unix timestamp string — only fetch messages after this.
    pub fn channel_history(&self, channel_id: &str, oldest: Option<&str>) -> Vec<serde_json::Value> {
        let mut messages = vec![];
        let mut cursor = String::new();

        loop {
            let mut params = vec![
                ("channel", channel_id),
                ("limit", "200"),
            ];
            if let Some(ts) = oldest {
                params.push(("oldest", ts));
            }
            if !cursor.is_empty() {
                params.push(("cursor", &cursor));
            }

            let data = match self.slack_get("https://slack.com/api/conversations.history", &params) {
                Some(d) => d,
                None => break,
            };

            if let Some(msgs) = data.get("messages").and_then(|m| m.as_array()) {
                messages.extend(msgs.clone());
            }

            let has_more = data.get("has_more").and_then(|v| v.as_bool()).unwrap_or(false);
            if !has_more { break; }

            cursor = data.get("response_metadata")
                .and_then(|m| m.get("next_cursor"))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            if cursor.is_empty() { break; }

            // Rate limit: Slack allows ~50 requests/minute for tier 3
            std::thread::sleep(std::time::Duration::from_millis(1200));
        }

        messages
    }

    /// Fetch thread replies.
    pub fn thread_replies(&self, channel_id: &str, thread_ts: &str) -> Vec<serde_json::Value> {
        let params = vec![
            ("channel", channel_id),
            ("ts", thread_ts),
            ("limit", "200"),
        ];
        match self.slack_get("https://slack.com/api/conversations.replies", &params) {
            Some(data) => data.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default(),
            None => vec![],
        }
    }

    /// Fetch user info (cached per session).
    pub fn user_info(&self, user_id: &str) -> Option<(String, String)> {
        let data = self.slack_get(
            "https://slack.com/api/users.info",
            &[("user", user_id)],
        )?;
        let user = data.get("user")?;
        let name = user.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let real_name = user.get("real_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        Some((name, real_name))
    }

    /// Ingest all channels and messages into the graph.
    /// `since_ts` = only ingest messages after this Unix timestamp (0 = full backfill).
    pub fn ingest(&self, graph: &GraphClient, since_ts: f64) -> IngestStats {
        let mut stats = IngestStats::default();

        // Get known service names from the graph for cross-referencing
        let known_services = self.get_known_services(graph);

        // Fetch channels
        let channels = self.list_channels();
        stats.channels = channels.len();

        for (ch_id, ch_name, member_count) in &channels {
            // Create channel node
            let _ = graph.query(
                &format!(
                    "MERGE (c:SlackChannel {{id: '{}'}}) \
                     SET c.name = '{}', c.member_count = {}, \
                     c.is_ops = {}",
                    escape(ch_id), escape(ch_name), member_count,
                    is_ops_channel(ch_name)
                ),
                &[],
            );

            // Fetch messages
            let oldest = if since_ts > 0.0 { Some(format!("{}", since_ts)) } else { None };
            let messages = self.channel_history(ch_id, oldest.as_deref());
            stats.messages += messages.len();

            for msg in &messages {
                self.ingest_message(graph, msg, ch_id, ch_name, &known_services, &mut stats);
            }

            println!("[slack] #{}: {} messages ingested", ch_name, messages.len());
        }

        stats
    }

    /// Ingest a single message into the graph.
    fn ingest_message(
        &self,
        graph: &GraphClient,
        msg: &serde_json::Value,
        channel_id: &str,
        channel_name: &str,
        known_services: &HashSet<String>,
        stats: &mut IngestStats,
    ) {
        let ts = msg.get("ts").and_then(|v| v.as_str()).unwrap_or("");
        let text = msg.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let user_id = msg.get("user").and_then(|v| v.as_str()).unwrap_or("");
        let thread_ts = msg.get("thread_ts").and_then(|v| v.as_str());
        let reply_count = msg.get("reply_count").and_then(|v| v.as_u64()).unwrap_or(0);

        if ts.is_empty() || text.is_empty() { return; }

        let timestamp: f64 = ts.parse().unwrap_or(0.0);
        let text_lower = text.to_lowercase();

        // Detect symptoms and resolutions
        let has_symptom = SYMPTOM_KEYWORDS.iter().any(|kw| text_lower.contains(kw));
        let is_resolution = RESOLUTION_KEYWORDS.iter().any(|kw| text_lower.contains(kw));
        let has_command = COMMAND_PATTERNS.iter().any(|kw| text_lower.contains(kw));

        // Truncate text for graph storage (keep first 500 chars)
        let text_short = if text.len() > 500 { &text[..500] } else { text };

        // Create message node
        let _ = graph.query(
            &format!(
                "MERGE (m:SlackMessage {{ts: '{}'}}) \
                 SET m.text = '{}', m.timestamp = {}, \
                 m.channel_id = '{}', m.channel_name = '{}', \
                 m.user_id = '{}', m.has_symptom = {}, \
                 m.is_resolution = {}, m.has_command = {}, \
                 m.reply_count = {}",
                escape(ts), escape(text_short), timestamp,
                escape(channel_id), escape(channel_name),
                escape(user_id), has_symptom, is_resolution, has_command,
                reply_count,
            ),
            &[],
        );

        // Edge: message → channel
        let _ = graph.query(
            &format!(
                "MATCH (m:SlackMessage {{ts: '{}'}}), (c:SlackChannel {{id: '{}'}}) \
                 MERGE (m)-[:IN_CHANNEL]->(c)",
                escape(ts), escape(channel_id)
            ),
            &[],
        );

        // Edge: message → user
        if !user_id.is_empty() {
            let _ = graph.query(
                &format!(
                    "MERGE (u:SlackUser {{id: '{}'}}) \
                     MERGE (m:SlackMessage {{ts: '{}'}}) \
                     MERGE (m)-[:SENT_BY]->(u)",
                    escape(user_id), escape(ts)
                ),
                &[],
            );
        }

        // Edge: message → thread
        if let Some(tts) = thread_ts {
            if tts != ts {
                // This is a reply in a thread
                let _ = graph.query(
                    &format!(
                        "MERGE (t:SlackThread {{ts: '{}'}}) \
                         MERGE (m:SlackMessage {{ts: '{}'}}) \
                         MERGE (m)-[:IN_THREAD]->(t)",
                        escape(tts), escape(ts)
                    ),
                    &[],
                );
            } else if reply_count > 0 {
                // This is the thread parent
                let _ = graph.query(
                    &format!(
                        "MERGE (t:SlackThread {{ts: '{}'}}) \
                         SET t.reply_count = {}, t.channel_id = '{}', t.channel_name = '{}'",
                        escape(ts), reply_count, escape(channel_id), escape(channel_name)
                    ),
                    &[],
                );
            }
        }

        // Cross-layer edges: message mentions known services
        for service_name in known_services {
            if text_lower.contains(&service_name.to_lowercase()) {
                let _ = graph.query(
                    &format!(
                        "MATCH (m:SlackMessage {{ts: '{}'}}), (s) \
                         WHERE (s:K8sService OR s:K8sDeployment) AND toLower(s.name) = '{}' \
                         MERGE (m)-[:MENTIONS_SERVICE]->(s)",
                        escape(ts), escape(&service_name.to_lowercase())
                    ),
                    &[],
                );
                stats.service_mentions += 1;
            }
        }

        // Temporal correlation: if message is near a K8s event (±5 min), create edge
        if has_symptom {
            stats.symptoms += 1;
            let _ = graph.query(
                &format!(
                    "MATCH (m:SlackMessage {{ts: '{}'}}), (e:K8sEvent) \
                     WHERE abs(e.timestamp - {}) < 300 \
                     MERGE (m)-[:DURING_EVENT]->(e)",
                    escape(ts), timestamp
                ),
                &[],
            );
        }

        if is_resolution {
            stats.resolutions += 1;
        }
    }

    /// Get known service/deployment names from the graph.
    fn get_known_services(&self, graph: &GraphClient) -> HashSet<String> {
        let mut services = HashSet::new();

        // K8s services
        if let Ok(r) = graph.query("MATCH (s:K8sService) RETURN s.name", &[]) {
            for row in &r.rows {
                let name = row[0].as_str();
                if !name.is_empty() {
                    services.insert(name.to_string());
                }
            }
        }

        // K8s deployments
        if let Ok(r) = graph.query("MATCH (d:K8sDeployment) RETURN d.name", &[]) {
            for row in &r.rows {
                let name = row[0].as_str();
                if !name.is_empty() {
                    services.insert(name.to_string());
                }
            }
        }

        // Also check other cluster graphs (dynamically discovered)
        for graph_name in &graph.discover_cluster_graphs() {
            if let Ok(g) = GraphClient::new(graph_name) {
                if let Ok(r) = g.query("MATCH (s:K8sService) RETURN s.name", &[]) {
                    for row in &r.rows {
                        let name = row[0].as_str();
                        if !name.is_empty() { services.insert(name.to_string()); }
                    }
                }
            }
        }

        services
    }

    /// Resolve user IDs to names and update graph.
    pub fn resolve_users(&self, graph: &GraphClient) {
        if let Ok(r) = graph.query(
            "MATCH (u:SlackUser) WHERE u.name IS NULL RETURN u.id LIMIT 50",
            &[],
        ) {
            for row in &r.rows {
                let uid = row[0].as_str();
                if uid.is_empty() { continue; }
                if let Some((name, real_name)) = self.user_info(uid) {
                    let _ = graph.query(
                        &format!(
                            "MATCH (u:SlackUser {{id: '{}'}}) SET u.name = '{}', u.real_name = '{}'",
                            escape(uid), escape(&name), escape(&real_name)
                        ),
                        &[],
                    );
                }
                // Rate limit
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }

    /// Detect activity staleness on all users and authors.
    /// Sets last_active_ts and active status so the graph knows who has current context
    /// vs who is historical. Never surfaces "inactive" as a performance metric,
    /// only used to recommend the right person to help fix a problem.
    pub fn update_activity_status(graph: &GraphClient) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let stale_threshold = now - (45.0 * 86400.0); // 45 days

        // Update SlackUser: find their most recent message timestamp
        let _ = graph.query(
            &format!(
                "MATCH (u:SlackUser)<-[:SENT_BY]-(m:SlackMessage) \
                 WITH u, max(m.timestamp) AS last_msg \
                 SET u.last_active_ts = last_msg, \
                 u.has_recent_activity = last_msg >= {}",
                stale_threshold
            ),
            &[],
        );

        // Update Author: find their most recent commit date
        let _ = graph.query(
            "MATCH (a:Author)-[:AUTHORED]->(c:Commit) \
             WITH a, max(c.date) AS last_commit \
             SET a.last_commit_date = last_commit",
            &[],
        );

        // For users with no recent activity, find who has context on their code
        // so the graph can suggest alternatives
        let _ = graph.query(
            &format!(
                "MATCH (u:SlackUser) WHERE u.has_recent_activity = false \
                 MATCH (m:SlackMessage)-[:SENT_BY]->(u) \
                 MATCH (m)-[:MENTIONS_SERVICE]->(svc) \
                 MATCH (m2:SlackMessage)-[:MENTIONS_SERVICE]->(svc) \
                 MATCH (m2)-[:SENT_BY]->(active:SlackUser) \
                 WHERE active <> u AND active.has_recent_activity = true \
                 WITH u, active, count(m2) AS shared_context \
                 ORDER BY shared_context DESC \
                 WITH u, collect(active.name)[0] AS suggested_replacement \
                 SET u.suggested_contact = suggested_replacement"
            ),
            &[],
        );
    }
}

#[derive(Default)]
pub struct IngestStats {
    pub channels: usize,
    pub messages: usize,
    pub symptoms: usize,
    pub resolutions: usize,
    pub service_mentions: usize,
}

impl IngestStats {
    pub fn summary(&self) -> String {
        format!(
            "{} channels, {} messages, {} symptoms, {} resolutions, {} service mentions",
            self.channels, self.messages, self.symptoms, self.resolutions, self.service_mentions
        )
    }
}

fn is_ops_channel(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("ops") || n.contains("infra") || n.contains("alert")
        || n.contains("incident") || n.contains("oncall") || n.contains("sre")
        || n.contains("deploy") || n.contains("prod")
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', "\\n").replace('\r', "")
}
