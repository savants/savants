#![allow(dead_code)]
//! Log intelligence pipeline for the Mazkir runtime layer.
//!
//! Streams pod logs via the kube API, classifies severity, extracts
//! templates (masking numbers/IPs/hex/UUIDs), deduplicates by
//! (pod, template_hash), and writes LogEvent nodes with EMITTED edges.
//!
//! This is a stub implementation — the core classifier and template
//! extraction are complete, but the streaming tail loop and graph write
//! will be filled in when watch mode is fully wired.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;

use crate::graph::GraphClient;

// ---------------------------------------------------------------------------
// Tier 1: severity classifier
// ---------------------------------------------------------------------------

/// Compiled severity patterns (order: most severe first).
struct SeverityClassifier {
    fatal: Regex,
    error: Regex,
    warn: Regex,
    high_signal: Regex,
    drop: Regex,
}

impl SeverityClassifier {
    fn new() -> Self {
        Self {
            fatal: Regex::new(r"\b(FATAL|PANIC|panic:)\b").unwrap(),
            error: Regex::new(r"(?i)\b(ERROR|ERR|Exception|Traceback|error:)\b").unwrap(),
            warn: Regex::new(r"(?i)\b(WARN|WARNING)\b").unwrap(),
            high_signal: Regex::new(concat!(
                r"(?i)\b(",
                r"OOMKilled|CrashLoopBackOff|ImagePullBackOff|",
                r"connection\s+refused|dial\s+tcp|",
                r"permission\s+denied|ENOENT|EACCES|",
                r"timeout|timed\s+out|",
                r"segmentation\s+fault|segfault|",
                r"5\d\d\s|",
                r"KeyError|ValueError|RuntimeError|NullPointerException|",
                r"no\s+such\s+file",
                r")"
            ))
            .unwrap(),
            drop: Regex::new(r"(?i)\b(DEBUG|TRACE|healthz|readyz|livez|/metrics\s)").unwrap(),
        }
    }

    /// Return a severity label if the line is significant, else None.
    fn classify(&self, line: &str) -> Option<&'static str> {
        if line.is_empty() || line.len() > 8192 {
            return None;
        }
        if self.drop.is_match(line) {
            return None;
        }
        if self.fatal.is_match(line) {
            return Some("FATAL");
        }
        if self.error.is_match(line) {
            return Some("ERROR");
        }
        if self.warn.is_match(line) {
            return Some("WARN");
        }
        if self.high_signal.is_match(line) {
            return Some("WARN");
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Tier 2: template extraction (mask numbers, IPs, hex, UUIDs)
// ---------------------------------------------------------------------------

struct TemplateExtractor {
    uuid: Regex,
    ip: Regex,
    hex: Regex,
    numbers: Regex,
}

impl TemplateExtractor {
    fn new() -> Self {
        Self {
            uuid: Regex::new(
                r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
            ).unwrap(),
            ip: Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}(:\d+)?\b").unwrap(),
            hex: Regex::new(r"\b0x[0-9a-fA-F]+\b").unwrap(),
            numbers: Regex::new(r"\b\d+\b").unwrap(),
        }
    }

    /// Mask variable parts to produce a template string.
    fn extract(&self, line: &str) -> String {
        let s = self.uuid.replace_all(line, "<UUID>");
        let s = self.ip.replace_all(&s, "<IP>");
        let s = self.hex.replace_all(&s, "<HEX>");
        let s = self.numbers.replace_all(&s, "<N>");
        s.to_string()
    }

    /// Simple hash of a template string for dedup keying.
    fn template_hash(template: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        template.hash(&mut h);
        format!("{:016x}", h.finish())
    }
}

// ---------------------------------------------------------------------------
// Bucket: per-(pod, template_hash) accumulator
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct LogBucket {
    template_hash: String,
    template_text: String,
    severity: String,
    first_seen: f64,
    last_seen: f64,
    count: u64,
    example_lines: Vec<String>,
    dirty: bool,
}

impl LogBucket {
    const EXAMPLE_CAP: usize = 5;

    fn new(template_hash: String, template_text: String, severity: &str, ts: f64) -> Self {
        Self {
            template_hash,
            template_text,
            severity: severity.to_string(),
            first_seen: ts,
            last_seen: ts,
            count: 0,
            example_lines: Vec::new(),
            dirty: false,
        }
    }

    fn add(&mut self, line: &str, severity: &str, ts: f64) {
        self.count += 1;
        self.last_seen = ts;
        if severity_rank(severity) > severity_rank(&self.severity) {
            self.severity = severity.to_string();
        }
        if self.example_lines.len() < Self::EXAMPLE_CAP {
            self.example_lines.push(line.to_string());
        }
        self.dirty = true;
    }
}

fn severity_rank(sev: &str) -> u8 {
    match sev {
        "INFO" => 0,
        "WARN" => 1,
        "ERROR" => 2,
        "FATAL" => 3,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Log intelligence controller
// ---------------------------------------------------------------------------

/// Top-level log intelligence manager.
///
/// This is a stub — call `ingest_line` to feed lines through the pipeline
/// and `flush` to write dirty buckets to the graph.
pub struct LogIntelligence {
    graph: GraphClient,
    cluster: String,
    classifier: SeverityClassifier,
    extractor: TemplateExtractor,
    /// Buckets keyed by (pod, namespace, template_hash)
    buckets: Arc<Mutex<HashMap<(String, String, String), LogBucket>>>,
}

impl LogIntelligence {
    pub fn new(graph: GraphClient, cluster: String) -> Self {
        Self {
            graph,
            cluster,
            classifier: SeverityClassifier::new(),
            extractor: TemplateExtractor::new(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Feed a single log line through the classification + template pipeline.
    /// Returns true if the line was significant and accumulated.
    pub fn ingest_line(&self, pod: &str, namespace: &str, line: &str) -> bool {
        let severity = match self.classifier.classify(line) {
            Some(s) => s,
            None => return false,
        };

        let template = self.extractor.extract(line);
        let hash = TemplateExtractor::template_hash(&template);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let key = (pod.to_string(), namespace.to_string(), hash.clone());
        let mut buckets = self.buckets.lock().unwrap();
        let bucket = buckets.entry(key).or_insert_with(|| {
            LogBucket::new(hash, template, severity, now)
        });
        bucket.add(line, severity, now);
        true
    }

    /// Flush all dirty buckets to the graph as LogEvent nodes + EMITTED edges.
    /// Returns the number of events flushed.
    pub fn flush(&self) -> u32 {
        let mut buckets = self.buckets.lock().unwrap();
        let mut n = 0u32;

        for ((pod, namespace, _), bucket) in buckets.iter_mut() {
            if !bucket.dirty {
                continue;
            }

            let count_s = bucket.count.to_string();
            let first_s = format!("{:.3}", bucket.first_seen);
            let last_s = format!("{:.3}", bucket.last_seen);
            let examples = bucket.example_lines.join("\\n");

            // MERGE LogEvent node
            let _ = self.graph.query(
                "MERGE (e:LogEvent {cluster: $cluster, namespace: $ns, pod: $pod, template_hash: $th}) \
                 SET e.severity = $sev, \
                     e.template_text = $tmpl, \
                     e.first_seen = $first, \
                     e.last_seen = $last, \
                     e.count = $count, \
                     e.example_lines = $examples",
                &[
                    ("cluster", self.cluster.as_str()),
                    ("ns", namespace.as_str()),
                    ("pod", pod.as_str()),
                    ("th", &bucket.template_hash),
                    ("sev", &bucket.severity),
                    ("tmpl", &bucket.template_text),
                    ("first", &first_s),
                    ("last", &last_s),
                    ("count", &count_s),
                    ("examples", &examples),
                ],
            );

            // EMITTED edge: Pod → LogEvent
            let _ = self.graph.query(
                "MATCH (p:K8sPod {name: $pod, namespace: $ns, cluster: $cluster}) \
                 MATCH (e:LogEvent {cluster: $cluster, namespace: $ns, pod: $pod, template_hash: $th}) \
                 MERGE (p)-[:EMITTED]->(e)",
                &[
                    ("pod", pod.as_str()),
                    ("ns", namespace.as_str()),
                    ("cluster", self.cluster.as_str()),
                    ("th", &bucket.template_hash),
                ],
            );

            // Entity index scan: check template text for known resource names
            // and create MENTIONS edges. This scans for ConfigMap/Secret/Service/
            // Deployment names that appear in the template or example lines.
            self.scan_entity_mentions(pod, namespace, &bucket.template_hash, &bucket.template_text, &bucket.example_lines);

            bucket.dirty = false;
            n += 1;
        }

        n
    }

    /// Scan template text for mentions of known K8s resource names in the same namespace.
    fn scan_entity_mentions(
        &self,
        pod: &str,
        namespace: &str,
        template_hash: &str,
        template_text: &str,
        example_lines: &[String],
    ) {
        // Query known entity names in this namespace
        for label in &["K8sConfigMap", "K8sSecret", "K8sService", "K8sDeployment"] {
            let cypher = format!(
                "MATCH (x:{} {{namespace: $ns, cluster: $cluster}}) RETURN x.name",
                label
            );
            if let Ok(result) = self.graph.query(
                &cypher,
                &[("ns", namespace), ("cluster", &self.cluster)],
            ) {
                for row in &result.rows {
                    let ent_name = row[0].as_str();
                    if ent_name.len() < 4 {
                        continue; // skip short names to avoid false positives
                    }
                    // Check if entity name appears in template or first example
                    let scan_text = if let Some(first) = example_lines.first() {
                        format!("{} {}", template_text, first)
                    } else {
                        template_text.to_string()
                    };
                    if scan_text.contains(ent_name) {
                        let cypher = format!(
                            "MATCH (e:LogEvent {{cluster: $cluster, namespace: $ns, pod: $pod, template_hash: $th}}) \
                             MATCH (x:{} {{name: $ent, namespace: $ns, cluster: $cluster}}) \
                             MERGE (e)-[:MENTIONS]->(x)",
                            label
                        );
                        let _ = self.graph.query(
                            &cypher,
                            &[
                                ("cluster", self.cluster.as_str()),
                                ("ns", namespace),
                                ("pod", pod),
                                ("th", template_hash),
                                ("ent", ent_name),
                            ],
                        );
                    }
                }
            }
        }
    }
}

