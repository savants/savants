//! Personal radar - identifies what YOU need to know across all channels
//! without reading everything. Detects direct mentions, implicit ownership,
//! semantic relevance, and situations where you should be involved but
//! nobody tagged you.
//!
//! This is observe-only. It never posts on your behalf.

use crate::graph::GraphClient;
use std::collections::HashSet;

pub struct PersonalRadar {
    /// The user's Slack user ID
    user_id: String,
    /// The user's display name (for matching in text)
    user_name: String,
    /// The user's git author name (for matching commits)
    git_author: String,
    /// The user's email (for Jira matching)
    email: String,
}

#[derive(Debug)]
pub struct RadarItem {
    pub urgency: Urgency,
    pub category: Category,
    pub channel: String,
    pub summary: String,
    pub timestamp: f64,
    pub from_user: String,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Urgency {
    Critical,  // prod error in your code, direct question unanswered
    High,      // someone discussing your service, ticket assigned to you discussed
    Medium,    // topic in your domain, you might have context
    Low,       // informational, your name mentioned casually
}

#[derive(Debug)]
pub enum Category {
    DirectMention,       // @you in the message
    UnansweredQuestion,  // someone asked you something, no reply
    YourCodeDiscussed,   // someone talking about code you own
    YourServiceDown,     // error in a service you maintain
    TicketDiscussion,    // your Jira ticket being discussed without you
    DomainRelevant,      // topic matches your expertise even though not tagged
    ActionNeeded,        // someone said they'd do something but it's directed at your area
}

impl PersonalRadar {
    pub fn new(user_id: String, user_name: String, git_author: String, email: String) -> Self {
        Self { user_id, user_name, git_author, email }
    }

    /// Build a PersonalRadar from the graph by matching a user identifier
    /// to their SlackUser, Author, and JiraTicket assignee nodes.
    pub fn from_graph(graph: &GraphClient, identifier: &str) -> Option<Self> {
        // Try to find the user by Slack name, email, or git author
        let id_lower = identifier.to_lowercase();

        let mut user_id = String::new();
        let mut user_name = String::new();
        let mut git_author = String::new();
        let mut email = String::new();

        // Search SlackUser nodes
        if let Ok(r) = graph.query(
            &format!(
                "MATCH (u:SlackUser) WHERE toLower(u.name) CONTAINS '{}' OR toLower(u.real_name) CONTAINS '{}' \
                 RETURN u.id, u.name, u.real_name LIMIT 1",
                esc(&id_lower), esc(&id_lower)
            ),
            &[],
        ) {
            if let Some(row) = r.rows.first() {
                user_id = row[0].as_str().to_string();
                user_name = row[1].as_str().to_string();
            }
        }

        // Search Author nodes
        if let Ok(r) = graph.query(
            &format!(
                "MATCH (a:Author) WHERE toLower(a.name) CONTAINS '{}' OR toLower(a.email) CONTAINS '{}' \
                 RETURN a.name, a.email LIMIT 1",
                esc(&id_lower), esc(&id_lower)
            ),
            &[],
        ) {
            if let Some(row) = r.rows.first() {
                git_author = row[0].as_str().to_string();
                email = row[1].as_str().to_string();
            }
        }

        if user_id.is_empty() && git_author.is_empty() {
            return None;
        }

        Some(Self { user_id, user_name, git_author, email })
    }

    /// Scan the graph for everything this user needs to know.
    /// Returns items sorted by urgency (critical first).
    pub fn scan(&self, graph: &GraphClient, since_hours: f64) -> Vec<RadarItem> {
        let mut items = vec![];
        let since_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64() - (since_hours * 3600.0);

        // Build the user's ownership profile from the graph
        let owned_services = self.get_owned_services(graph);
        let owned_files = self.get_owned_files(graph);
        let assigned_tickets = self.get_assigned_tickets(graph);

        // 1. Direct mentions not replied to
        self.check_direct_mentions(graph, since_ts, &mut items);

        // 2. Unanswered questions directed at you
        self.check_unanswered_questions(graph, since_ts, &mut items);

        // 3. Your code being discussed (someone talking about files/functions you own)
        self.check_code_discussions(graph, since_ts, &owned_files, &owned_services, &mut items);

        // 4. Production errors in your services
        self.check_service_errors(graph, since_ts, &owned_services, &mut items);

        // 5. Your Jira tickets being discussed without you
        self.check_ticket_discussions(graph, since_ts, &assigned_tickets, &mut items);

        // 6. Domain-relevant discussions you should know about
        self.check_domain_relevance(graph, since_ts, &owned_services, &owned_files, &mut items);

        // Sort by urgency (Critical first)
        items.sort_by(|a, b| a.urgency.cmp(&b.urgency));

        items
    }

    /// Get services this user owns (based on code modification frequency)
    fn get_owned_services(&self, graph: &GraphClient) -> HashSet<String> {
        let mut services = HashSet::new();

        // Services where this user has the most commits
        if !self.git_author.is_empty() {
            if let Ok(r) = graph.query(
                &format!(
                    "MATCH (a:Author {{name: '{}'}})-[:AUTHORED]->(c:Commit)-[:MODIFIED]->(f:CodeFunction) \
                     RETURN DISTINCT f.file",
                    esc(&self.git_author)
                ),
                &[],
            ) {
                for row in &r.rows {
                    let file = row[0].as_str();
                    // Extract service name from file path (e.g., "server/services/identity-verification.ts" -> "identity-verification")
                    if let Some(name) = file.split('/').last() {
                        let service = name.replace(".ts", "").replace(".js", "").replace(".tsx", "").replace(".py", "");
                        if service.len() > 3 {
                            services.insert(service);
                        }
                    }
                }
            }
        }

        services
    }

    /// Get files this user has modified recently
    fn get_owned_files(&self, graph: &GraphClient) -> HashSet<String> {
        let mut files = HashSet::new();

        if !self.git_author.is_empty() {
            if let Ok(r) = graph.query(
                &format!(
                    "MATCH (a:Author {{name: '{}'}})-[:AUTHORED]->(c:Commit)-[:MODIFIED_FILE]->(f:CodeFile) \
                     RETURN DISTINCT f.path LIMIT 50",
                    esc(&self.git_author)
                ),
                &[],
            ) {
                for row in &r.rows {
                    files.insert(row[0].as_str().to_string());
                }
            }
        }

        files
    }

    /// Get Jira tickets assigned to this user
    fn get_assigned_tickets(&self, graph: &GraphClient) -> HashSet<String> {
        let mut tickets = HashSet::new();

        let search_names = vec![&self.user_name, &self.git_author];
        for name in search_names {
            if name.is_empty() { continue; }
            if let Ok(r) = graph.query(
                &format!(
                    "MATCH (t:IssueTicket) WHERE toLower(t.assignee) CONTAINS '{}' RETURN t.key",
                    esc(&name.to_lowercase())
                ),
                &[],
            ) {
                for row in &r.rows {
                    tickets.insert(row[0].as_str().to_string());
                }
            }
            // Also try JiraTicket label (might not have been renamed yet)
            if let Ok(r) = graph.query(
                &format!(
                    "MATCH (t:JiraTicket) WHERE toLower(t.assignee) CONTAINS '{}' RETURN t.key",
                    esc(&name.to_lowercase())
                ),
                &[],
            ) {
                for row in &r.rows {
                    tickets.insert(row[0].as_str().to_string());
                }
            }
        }

        tickets
    }

    /// Check for direct @mentions with no reply from this user
    fn check_direct_mentions(&self, graph: &GraphClient, since_ts: f64, items: &mut Vec<RadarItem>) {
        if self.user_id.is_empty() { return; }

        // Messages that tag this user
        let mention_tag = format!("<@{}>", self.user_id);
        if let Ok(r) = graph.query(
            &format!(
                "MATCH (m:SlackMessage)-[:SENT_BY]->(sender:SlackUser) \
                 WHERE m.text CONTAINS '{}' AND m.timestamp >= {} AND sender.id <> '{}' \
                 AND m.reply_count = 0 \
                 RETURN sender.name, m.channel_name, m.text, m.timestamp \
                 ORDER BY m.timestamp DESC LIMIT 20",
                esc(&mention_tag), since_ts, esc(&self.user_id)
            ),
            &[],
        ) {
            for row in &r.rows {
                let text = row[2].as_str();
                let has_question = text.contains('?');
                items.push(RadarItem {
                    urgency: if has_question { Urgency::Critical } else { Urgency::High },
                    category: if has_question { Category::UnansweredQuestion } else { Category::DirectMention },
                    channel: row[1].as_str().to_string(),
                    summary: format!("@{}: {}", row[0].as_str(), truncate(text, 120)),
                    timestamp: row[3].as_f64(),
                    from_user: row[0].as_str().to_string(),
                });
            }
        }
    }

    /// Check for questions directed at this user with no reply
    fn check_unanswered_questions(&self, graph: &GraphClient, since_ts: f64, items: &mut Vec<RadarItem>) {
        if self.user_name.is_empty() { return; }

        // Messages mentioning the user's name (not just @tag) with a question mark
        if let Ok(r) = graph.query(
            &format!(
                "MATCH (m:SlackMessage)-[:SENT_BY]->(sender:SlackUser) \
                 WHERE toLower(m.text) CONTAINS '{}' AND m.text CONTAINS '?' \
                 AND m.timestamp >= {} AND m.reply_count = 0 \
                 AND sender.name <> '{}' \
                 RETURN sender.name, m.channel_name, m.text, m.timestamp \
                 ORDER BY m.timestamp DESC LIMIT 10",
                esc(&self.user_name.to_lowercase()), since_ts, esc(&self.user_name)
            ),
            &[],
        ) {
            for row in &r.rows {
                items.push(RadarItem {
                    urgency: Urgency::Critical,
                    category: Category::UnansweredQuestion,
                    channel: row[1].as_str().to_string(),
                    summary: format!("@{} asked you: {}", row[0].as_str(), truncate(row[2].as_str(), 120)),
                    timestamp: row[3].as_f64(),
                    from_user: row[0].as_str().to_string(),
                });
            }
        }
    }

    /// Check for discussions about code this user owns
    fn check_code_discussions(&self, graph: &GraphClient, since_ts: f64, owned_files: &HashSet<String>, owned_services: &HashSet<String>, items: &mut Vec<RadarItem>) {
        // Check if anyone is discussing files/services this user owns
        for service in owned_services {
            if service.len() < 5 { continue; }
            if let Ok(r) = graph.query(
                &format!(
                    "MATCH (m:SlackMessage)-[:SENT_BY]->(sender:SlackUser) \
                     WHERE toLower(m.text) CONTAINS '{}' AND m.timestamp >= {} \
                     AND m.has_symptom = true AND sender.id <> '{}' \
                     RETURN sender.name, m.channel_name, m.text, m.timestamp \
                     ORDER BY m.timestamp DESC LIMIT 5",
                    esc(&service.to_lowercase()), since_ts, esc(&self.user_id)
                ),
                &[],
            ) {
                for row in &r.rows {
                    items.push(RadarItem {
                        urgency: Urgency::High,
                        category: Category::YourCodeDiscussed,
                        channel: row[1].as_str().to_string(),
                        summary: format!("Issue discussed about {} (your code): {}", service, truncate(row[2].as_str(), 100)),
                        timestamp: row[3].as_f64(),
                        from_user: row[0].as_str().to_string(),
                    });
                }
            }
        }
    }

    /// Check for production errors in services this user maintains
    fn check_service_errors(&self, graph: &GraphClient, since_ts: f64, owned_services: &HashSet<String>, items: &mut Vec<RadarItem>) {
        for service in owned_services {
            if let Ok(r) = graph.query(
                &format!(
                    "MATCH (si:SentryIssue)-[:CRASHES_IN]->(f:CodeFunction) \
                     WHERE toLower(f.file) CONTAINS '{}' AND si.level = 'error' \
                     RETURN si.title, si.count, f.file, f.name LIMIT 3",
                    esc(&service.to_lowercase())
                ),
                &[],
            ) {
                for row in &r.rows {
                    let count = row[1].as_i64();
                    items.push(RadarItem {
                        urgency: if count > 10 { Urgency::Critical } else { Urgency::High },
                        category: Category::YourServiceDown,
                        channel: "sentry".to_string(),
                        summary: format!("{} (x{}) in {} - your code", row[0].as_str(), count, row[3].as_str()),
                        timestamp: 0.0,
                        from_user: "sentry".to_string(),
                    });
                }
            }
        }
    }

    /// Check for Jira ticket discussions the user isn't part of
    fn check_ticket_discussions(&self, graph: &GraphClient, since_ts: f64, assigned_tickets: &HashSet<String>, items: &mut Vec<RadarItem>) {
        for ticket_key in assigned_tickets {
            if let Ok(r) = graph.query(
                &format!(
                    "MATCH (m:SlackMessage)-[:SENT_BY]->(sender:SlackUser) \
                     WHERE m.text CONTAINS '{}' AND m.timestamp >= {} \
                     AND sender.id <> '{}' \
                     RETURN sender.name, m.channel_name, m.text, m.timestamp \
                     ORDER BY m.timestamp DESC LIMIT 3",
                    esc(ticket_key), since_ts, esc(&self.user_id)
                ),
                &[],
            ) {
                for row in &r.rows {
                    items.push(RadarItem {
                        urgency: Urgency::Medium,
                        category: Category::TicketDiscussion,
                        channel: row[1].as_str().to_string(),
                        summary: format!("{} (assigned to you) discussed by @{}: {}", ticket_key, row[0].as_str(), truncate(row[2].as_str(), 80)),
                        timestamp: row[3].as_f64(),
                        from_user: row[0].as_str().to_string(),
                    });
                }
            }
        }
    }

    /// Check for domain-relevant discussions where the user should be involved
    /// but wasn't tagged. This is the "you should know about this" detector.
    fn check_domain_relevance(&self, graph: &GraphClient, since_ts: f64, owned_services: &HashSet<String>, owned_files: &HashSet<String>, items: &mut Vec<RadarItem>) {
        // Find messages about topics this user has expertise in but wasn't mentioned
        // Strategy: messages with symptoms that mention functions/files this user owns,
        // but don't tag this user and aren't in channels the user typically participates in

        if self.user_id.is_empty() { return; }

        // Get channels the user is active in
        let mut active_channels = HashSet::new();
        if let Ok(r) = graph.query(
            &format!(
                "MATCH (m:SlackMessage)-[:SENT_BY]->(u:SlackUser {{id: '{}'}}) \
                 RETURN DISTINCT m.channel_name",
                esc(&self.user_id)
            ),
            &[],
        ) {
            for row in &r.rows {
                active_channels.insert(row[0].as_str().to_string());
            }
        }

        // Find symptom messages in channels the user doesn't frequent
        // that mention services/code the user owns
        for service in owned_services {
            if service.len() < 5 { continue; }
            if let Ok(r) = graph.query(
                &format!(
                    "MATCH (m:SlackMessage)-[:SENT_BY]->(sender:SlackUser) \
                     WHERE toLower(m.text) CONTAINS '{}' AND m.timestamp >= {} \
                     AND NOT m.text CONTAINS '<@{}>' \
                     AND sender.id <> '{}' \
                     AND (m.has_symptom = true OR m.text CONTAINS '?') \
                     RETURN sender.name, m.channel_name, m.text, m.timestamp \
                     ORDER BY m.timestamp DESC LIMIT 5",
                    esc(&service.to_lowercase()), since_ts,
                    esc(&self.user_id), esc(&self.user_id)
                ),
                &[],
            ) {
                for row in &r.rows {
                    let channel = row[1].as_str();
                    // Only flag if it's a channel the user doesn't normally participate in
                    // OR if it's a symptom (error report) even in their own channel
                    let is_foreign_channel = !active_channels.contains(channel);
                    if is_foreign_channel || row[2].as_str().to_lowercase().contains("error") || row[2].as_str().to_lowercase().contains("broken") {
                        items.push(RadarItem {
                            urgency: Urgency::Medium,
                            category: Category::DomainRelevant,
                            channel: channel.to_string(),
                            summary: format!("You weren't tagged but {} relates to your work: {}", service, truncate(row[2].as_str(), 100)),
                            timestamp: row[3].as_f64(),
                            from_user: row[0].as_str().to_string(),
                        });
                    }
                }
            }
        }
    }

    /// Format the radar output for display
    pub fn format_digest(&self, items: &[RadarItem]) -> String {
        if items.is_empty() {
            return "Nothing needs your attention right now.".to_string();
        }

        let mut output = vec![format!("{} items need your attention:\n", items.len())];

        let mut current_urgency = None;
        for item in items {
            if current_urgency != Some(&item.urgency) {
                current_urgency = Some(&item.urgency);
                let label = match item.urgency {
                    Urgency::Critical => "CRITICAL - respond now",
                    Urgency::High => "HIGH - review today",
                    Urgency::Medium => "MEDIUM - when you have time",
                    Urgency::Low => "LOW - informational",
                };
                output.push(format!("\n{}:", label));
            }

            let cat_label = match item.category {
                Category::DirectMention => "mentioned you",
                Category::UnansweredQuestion => "waiting for your answer",
                Category::YourCodeDiscussed => "your code discussed",
                Category::YourServiceDown => "your service has errors",
                Category::TicketDiscussion => "your ticket discussed",
                Category::DomainRelevant => "relates to your work",
                Category::ActionNeeded => "action needed",
            };

            output.push(format!("  [{}] #{}: {}", cat_label, item.channel, item.summary));
        }

        output.join("\n")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() } else { format!("{}...", &s[..max]) }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', " ").replace('\r', "")
}
