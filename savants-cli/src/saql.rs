//! SaQL: Savants Query Language
//!
//! A resource-oriented query language that hides the storage engine.
//! The AI agent sends SaQL; Savants translates to internal queries.
//! Nobody outside this module knows what database is underneath.
//!
//! Grammar:
//!   show <resource_type> [where <field> = <value> ...] [in <scope>] [limit N]
//!   story <resource_type> <name> [in <scope>] [since <duration>] [severity <level>]
//!   causes of <resource_type> <name> [in <scope>]
//!   dependents of <resource_type> <name> [in <scope>]
//!   impact of <resource_type> <name> [in <file>]
//!   trace <resource_type> <name> to <target_type>
//!   stats
//!
//! Resource types: pods, deployments, services, configmaps, secrets,
//!   hosts, processes, units, containers, functions, classes, files
//!
//! Durations: 5m, 1h, 24h, 7d
//! Severities: INFO, WARN, ERROR, FATAL

use crate::graph::GraphClient;
use std::collections::HashMap;

/// A parsed SaQL query.
#[derive(Debug)]
pub enum SaqlQuery {
    Show {
        resource: ResourceType,
        filters: Vec<Filter>,
        scope: Option<String>,
        limit: usize,
    },
    Story {
        resource: ResourceType,
        name: String,
        scope: Option<String>,
        since_minutes: u64,
        severity: String,
    },
    Causes {
        resource: ResourceType,
        name: String,
        scope: Option<String>,
    },
    Dependents {
        resource: ResourceType,
        name: String,
        scope: Option<String>,
    },
    Impact {
        resource: ResourceType,
        name: String,
        file: Option<String>,
    },
    Trace {
        resource: ResourceType,
        name: String,
        target: ResourceType,
    },
    Stats,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResourceType {
    Pod,
    Deployment,
    Service,
    ConfigMap,
    Secret,
    Namespace,
    Host,
    Process,
    SystemdUnit,
    DockerContainer,
    Function,
    Class,
    File,
    LogEvent,
    HostLogEvent,
}

impl ResourceType {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "pod" | "pods" => Some(Self::Pod),
            "deployment" | "deployments" | "deploy" => Some(Self::Deployment),
            "service" | "services" | "svc" => Some(Self::Service),
            "configmap" | "configmaps" | "cm" => Some(Self::ConfigMap),
            "secret" | "secrets" => Some(Self::Secret),
            "namespace" | "namespaces" | "ns" => Some(Self::Namespace),
            "host" | "hosts" | "node" | "nodes" => Some(Self::Host),
            "process" | "processes" | "proc" => Some(Self::Process),
            "unit" | "units" | "service-unit" => Some(Self::SystemdUnit),
            "container" | "containers" => Some(Self::DockerContainer),
            "function" | "functions" | "fn" | "func" => Some(Self::Function),
            "class" | "classes" => Some(Self::Class),
            "file" | "files" => Some(Self::File),
            "log" | "logs" | "event" | "events" => Some(Self::LogEvent),
            "hostlog" | "hostlogs" | "journal" => Some(Self::HostLogEvent),
            _ => None,
        }
    }

    /// The internal node label — NEVER exposed to the user/agent.
    fn label(&self) -> &'static str {
        match self {
            Self::Pod => "K8sPod",
            Self::Deployment => "K8sDeployment",
            Self::Service => "K8sService",
            Self::ConfigMap => "K8sConfigMap",
            Self::Secret => "K8sSecret",
            Self::Namespace => "K8sNamespace",
            Self::Host => "Host",
            Self::Process => "HostProcess",
            Self::SystemdUnit => "SystemdUnit",
            Self::DockerContainer => "DockerContainer",
            Self::Function => "Function",
            Self::Class => "Class",
            Self::File => "File",
            Self::LogEvent => "LogEvent",
            Self::HostLogEvent => "HostLogEvent",
        }
    }
}

#[derive(Debug)]
pub struct Filter {
    pub field: String,
    pub value: String,
}

/// Parse a SaQL query string into a structured query.
pub fn parse(input: &str) -> Result<SaqlQuery, String> {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    if tokens.is_empty() {
        return Err("Empty query".into());
    }

    match tokens[0].to_lowercase().as_str() {
        "show" | "list" | "get" => parse_show(&tokens[1..]),
        "story" | "describe" | "explain" => parse_story(&tokens[1..]),
        "causes" => parse_causes(&tokens[1..]),
        "dependents" | "dependencies" | "deps" => parse_dependents(&tokens[1..]),
        "impact" => parse_impact(&tokens[1..]),
        "trace" | "follow" => parse_trace(&tokens[1..]),
        "stats" | "status" => Ok(SaqlQuery::Stats),
        _ => Err(format!("Unknown command: '{}'. Try: show, story, causes, dependents, impact, trace, stats", tokens[0])),
    }
}

fn parse_show(tokens: &[&str]) -> Result<SaqlQuery, String> {
    if tokens.is_empty() {
        return Err("show requires a resource type. Example: show pods".into());
    }
    let resource = ResourceType::from_str(tokens[0])
        .ok_or_else(|| format!("Unknown resource: '{}'", tokens[0]))?;

    let mut filters = Vec::new();
    let mut scope = None;
    let mut limit = 50;
    let mut i = 1;

    while i < tokens.len() {
        match tokens[i].to_lowercase().as_str() {
            "where" | "with" => {
                i += 1;
                while i + 2 <= tokens.len() {
                    if tokens.get(i + 1).map(|t| *t) == Some("=") {
                        filters.push(Filter {
                            field: tokens[i].to_string(),
                            value: tokens.get(i + 2).unwrap_or(&"").to_string(),
                        });
                        i += 3;
                    } else {
                        break;
                    }
                }
            }
            "in" | "from" | "cluster" => {
                i += 1;
                if i < tokens.len() {
                    scope = Some(tokens[i].to_string());
                    i += 1;
                }
            }
            "limit" => {
                i += 1;
                if i < tokens.len() {
                    limit = tokens[i].parse().unwrap_or(50);
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }

    Ok(SaqlQuery::Show { resource, filters, scope, limit })
}

fn parse_story(tokens: &[&str]) -> Result<SaqlQuery, String> {
    if tokens.len() < 2 {
        return Err("story requires: story <type> <name>. Example: story pod api-gateway".into());
    }
    let resource = ResourceType::from_str(tokens[0])
        .ok_or_else(|| format!("Unknown resource: '{}'", tokens[0]))?;
    let name = tokens[1].to_string();

    let mut scope = None;
    let mut since_minutes = 60;
    let mut severity = "WARN".to_string();
    let mut i = 2;

    while i < tokens.len() {
        match tokens[i].to_lowercase().as_str() {
            "in" | "from" | "cluster" => {
                i += 1;
                if i < tokens.len() { scope = Some(tokens[i].to_string()); i += 1; }
            }
            "since" => {
                i += 1;
                if i < tokens.len() {
                    since_minutes = parse_duration(tokens[i]);
                    i += 1;
                }
            }
            "severity" => {
                i += 1;
                if i < tokens.len() { severity = tokens[i].to_uppercase(); i += 1; }
            }
            _ => i += 1,
        }
    }

    Ok(SaqlQuery::Story { resource, name, scope, since_minutes, severity })
}

fn parse_causes(tokens: &[&str]) -> Result<SaqlQuery, String> {
    // "causes of pod api-gateway in prod"
    let tokens = skip_word(tokens, "of");
    if tokens.len() < 2 {
        return Err("causes requires: causes of <type> <name>".into());
    }
    let resource = ResourceType::from_str(tokens[0])
        .ok_or_else(|| format!("Unknown resource: '{}'", tokens[0]))?;
    let name = tokens[1].to_string();
    let scope = find_scope(&tokens[2..]);
    Ok(SaqlQuery::Causes { resource, name, scope })
}

fn parse_dependents(tokens: &[&str]) -> Result<SaqlQuery, String> {
    let tokens = skip_word(tokens, "of");
    if tokens.len() < 2 {
        return Err("dependents requires: dependents of <type> <name>".into());
    }
    let resource = ResourceType::from_str(tokens[0])
        .ok_or_else(|| format!("Unknown resource: '{}'", tokens[0]))?;
    let name = tokens[1].to_string();
    let scope = find_scope(&tokens[2..]);
    Ok(SaqlQuery::Dependents { resource, name, scope })
}

fn parse_impact(tokens: &[&str]) -> Result<SaqlQuery, String> {
    let tokens = skip_word(tokens, "of");
    if tokens.len() < 2 {
        return Err("impact requires: impact of <type> <name>".into());
    }
    let resource = ResourceType::from_str(tokens[0])
        .ok_or_else(|| format!("Unknown resource: '{}'", tokens[0]))?;
    let name = tokens[1].to_string();
    let file = find_value(&tokens[2..], "in");
    Ok(SaqlQuery::Impact { resource, name, file })
}

fn parse_trace(tokens: &[&str]) -> Result<SaqlQuery, String> {
    if tokens.len() < 4 {
        return Err("trace requires: trace <type> <name> to <target_type>".into());
    }
    let resource = ResourceType::from_str(tokens[0])
        .ok_or_else(|| format!("Unknown resource: '{}'", tokens[0]))?;
    let name = tokens[1].to_string();
    // skip "to"
    let target_idx = tokens.iter().position(|t| t.to_lowercase() == "to")
        .ok_or("trace requires 'to': trace pod api-gw to code")?;
    let target = ResourceType::from_str(tokens.get(target_idx + 1).unwrap_or(&""))
        .ok_or("Unknown target resource type")?;
    Ok(SaqlQuery::Trace { resource, name, target })
}

fn parse_duration(s: &str) -> u64 {
    let s = s.trim();
    if s.ends_with('m') {
        s.trim_end_matches('m').parse().unwrap_or(60)
    } else if s.ends_with('h') {
        s.trim_end_matches('h').parse::<u64>().unwrap_or(1) * 60
    } else if s.ends_with('d') {
        s.trim_end_matches('d').parse::<u64>().unwrap_or(1) * 1440
    } else {
        s.parse().unwrap_or(60)
    }
}

fn skip_word<'a>(tokens: &'a [&str], word: &str) -> &'a [&'a str] {
    if tokens.first().map(|t| t.to_lowercase() == word).unwrap_or(false) {
        &tokens[1..]
    } else {
        tokens
    }
}

fn find_scope(tokens: &[&str]) -> Option<String> {
    find_value(tokens, "in").or_else(|| find_value(tokens, "from"))
}

fn find_value(tokens: &[&str], keyword: &str) -> Option<String> {
    for (i, t) in tokens.iter().enumerate() {
        if t.to_lowercase() == keyword {
            return tokens.get(i + 1).map(|s| s.to_string());
        }
    }
    None
}

/// Execute a SaQL query against the graph and return formatted results.
/// This is the ONLY function that touches the internal query engine.
/// Everything above is pure parsing — no storage awareness.
pub fn execute(query: &SaqlQuery, client: &GraphClient) -> Result<String, String> {
    match query {
        SaqlQuery::Stats => {
            let nodes = client.query("MATCH (n) RETURN count(n)", &[])
                .map_err(|e| e.to_string())?;
            let edges = client.query("MATCH ()-[r]->() RETURN count(r)", &[])
                .map_err(|e| e.to_string())?;
            let n = nodes.rows.first().map(|r| r[0].as_i64()).unwrap_or(0);
            let e = edges.rows.first().map(|r| r[0].as_i64()).unwrap_or(0);
            Ok(format!("Resources: {}, Connections: {}", n, e))
        }

        SaqlQuery::Show { resource, filters, scope, limit } => {
            let label = resource.label();
            let mut where_clauses = Vec::new();
            let mut params: Vec<(&str, &str)> = Vec::new();

            for f in filters {
                // Map user-friendly field names to internal property names
                let prop = map_field_name(resource, &f.field);
                where_clauses.push(format!("n.{} = '{}'", prop, f.value.replace('\'', "\\'")));
            }

            let where_str = if where_clauses.is_empty() {
                String::new()
            } else {
                format!(" WHERE {}", where_clauses.join(" AND "))
            };

            let q = format!(
                "MATCH (n:{}){}  RETURN n ORDER BY n.name LIMIT {}",
                label, where_str, limit
            );

            let rows = client.query(&q, &[]).map_err(|e| e.to_string())?;
            if rows.rows.is_empty() {
                return Ok(format!("No {} found.", resource_plural(resource)));
            }

            let mut lines = vec![format!("Found {} {}:", rows.rows.len(), resource_plural(resource))];
            for row in &rows.rows {
                lines.push(format!("  {}", row.first().map(|v| v.as_str()).unwrap_or("?")));
            }
            Ok(lines.join("\n"))
        }

        // Other query types delegate to existing MCP tool implementations
        // The full implementation would translate each SaQL variant into
        // the appropriate internal queries using schema.rs constants.
        _ => Ok(format!("SaQL query type {:?} — full execution coming soon.",
            std::mem::discriminant(query))),
    }
}

fn map_field_name(resource: &ResourceType, field: &str) -> String {
    // Map user-friendly names to internal property names
    match field.to_lowercase().as_str() {
        "status" | "state" => match resource {
            ResourceType::Pod => "status".into(),
            ResourceType::SystemdUnit => "active_state".into(),
            ResourceType::DockerContainer => "state".into(),
            _ => field.to_string(),
        },
        "restarts" | "restart_count" => "restart_count".into(),
        "image" => "image".into(),
        "severity" | "level" => "severity".into(),
        "name" => "name".into(),
        "namespace" | "ns" => "namespace".into(),
        _ => field.to_string(),
    }
}

fn resource_plural(r: &ResourceType) -> &'static str {
    match r {
        ResourceType::Pod => "pods",
        ResourceType::Deployment => "deployments",
        ResourceType::Service => "services",
        ResourceType::ConfigMap => "configmaps",
        ResourceType::Secret => "secrets",
        ResourceType::Namespace => "namespaces",
        ResourceType::Host => "hosts",
        ResourceType::Process => "processes",
        ResourceType::SystemdUnit => "systemd units",
        ResourceType::DockerContainer => "containers",
        ResourceType::Function => "functions",
        ResourceType::Class => "classes",
        ResourceType::File => "files",
        ResourceType::LogEvent => "log events",
        ResourceType::HostLogEvent => "host log events",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_show_pods() {
        let q = parse("show pods").unwrap();
        match q {
            SaqlQuery::Show { resource, .. } => assert_eq!(resource, ResourceType::Pod),
            _ => panic!("Expected Show"),
        }
    }

    #[test]
    fn parse_show_with_filter() {
        let q = parse("show pods where status = CrashLoopBackOff").unwrap();
        match q {
            SaqlQuery::Show { filters, .. } => {
                assert_eq!(filters.len(), 1);
                assert_eq!(filters[0].field, "status");
                assert_eq!(filters[0].value, "CrashLoopBackOff");
            }
            _ => panic!("Expected Show"),
        }
    }

    #[test]
    fn parse_story() {
        let q = parse("story pod api-gateway in prod since 1h severity ERROR").unwrap();
        match q {
            SaqlQuery::Story { resource, name, scope, since_minutes, severity } => {
                assert_eq!(resource, ResourceType::Pod);
                assert_eq!(name, "api-gateway");
                assert_eq!(scope, Some("prod".into()));
                assert_eq!(since_minutes, 60);
                assert_eq!(severity, "ERROR");
            }
            _ => panic!("Expected Story"),
        }
    }

    #[test]
    fn parse_causes() {
        let q = parse("causes of pod api-gateway in prod").unwrap();
        match q {
            SaqlQuery::Causes { resource, name, scope } => {
                assert_eq!(resource, ResourceType::Pod);
                assert_eq!(name, "api-gateway");
                assert_eq!(scope, Some("prod".into()));
            }
            _ => panic!("Expected Causes"),
        }
    }

    #[test]
    fn parse_duration_formats() {
        assert_eq!(parse_duration("5m"), 5);
        assert_eq!(parse_duration("1h"), 60);
        assert_eq!(parse_duration("24h"), 1440);
        assert_eq!(parse_duration("7d"), 10080);
    }
}
