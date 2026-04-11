//! Writes kernel security events to the Savants graph.

use crate::events::KernelSecurityEvent;
use redis::{Client, Commands};

pub struct GraphWriter {
    client: Client,
    hostname: String,
    graph_name: String,
}

impl GraphWriter {
    pub fn new(hostname: &str, port: u16, graph_name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let url = format!("redis://localhost:{}", port);
        let client = Client::open(url)?;
        // Test connection
        let mut conn = client.get_connection()?;
        let _: String = redis::cmd("PING").query(&mut conn)?;
        Ok(Self {
            client,
            hostname: hostname.to_string(),
            graph_name: graph_name.to_string(),
        })
    }

    pub fn write_event(&self, event: &KernelSecurityEvent) {
        let template_hash = format!("{:x}", md5_hash(&format!("{}{}", event.probe, event.comm)));
        let severity = format!("{:?}", event.severity).to_uppercase();
        let detail_text = event.format();

        // MERGE the event into the graph
        let query = format!(
            "MERGE (e:KernelSecurityEvent {{hostname: '{}', probe: '{}', comm: '{}', template_hash: '{}'}}) \
             SET e.severity = '{}', e.last_seen = {}, e.pid = {}, e.uid = {}, \
             e.detail = '{}', e.count = CASE WHEN e.count IS NULL THEN 1 ELSE e.count + 1 END",
            escape(&self.hostname),
            escape(&event.probe),
            escape(&event.comm),
            escape(&template_hash),
            escape(&severity),
            event.timestamp.timestamp() as f64,
            event.pid,
            event.uid,
            escape(&detail_text),
        );

        if let Ok(mut conn) = self.client.get_connection() {
            let _: Result<(), _> = redis::cmd("GRAPH.QUERY")
                .arg(&self.graph_name)
                .arg(&format!("CYPHER {}", query))
                .query(&mut conn);
        }

        // Edge: Host DETECTED KernelSecurityEvent
        let edge_query = format!(
            "MATCH (h:Host {{hostname: '{}'}}) \
             MATCH (e:KernelSecurityEvent {{hostname: '{}', template_hash: '{}'}}) \
             MERGE (h)-[:DETECTED]->(e)",
            escape(&self.hostname),
            escape(&self.hostname),
            escape(&template_hash),
        );

        if let Ok(mut conn) = self.client.get_connection() {
            let _: Result<(), _> = redis::cmd("GRAPH.QUERY")
                .arg(&self.graph_name)
                .arg(&format!("CYPHER {}", edge_query))
                .query(&mut conn);
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('\'', "\\'").replace('\\', "\\\\")
}

fn md5_hash(s: &str) -> u64 {
    // Simple hash for deduplication — not cryptographic
    let mut hash: u64 = 5381;
    for b in s.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u64);
    }
    hash
}
