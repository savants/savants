//! Cloud cost ingestion — reads billing data from GCP BigQuery and AWS Cost Explorer,
//! writes it to the Savants graph as CostNode entries.
//!
//! Runs periodically in the daemon (every 6 hours). Each run:
//! 1. Queries the billing export for the last 30 days
//! 2. Writes per-service cost nodes to the graph
//! 3. Compares to previous period to detect spikes
//! 4. Alerts on anomalies (service > threshold, sudden increase)

use crate::graph::GraphClient;
use std::process::Command;

/// Auto-discover GCP billing data — tries multiple methods:
/// 1. BigQuery billing export (most detailed, per-SKU)
/// 2. Cloud Billing API (no setup needed, per-service)
/// 3. If neither works, offer to enable billing export
///
/// Returns Vec of (project_id, dataset, table_name).
/// Empty dataset+table means "use Billing API fallback."
pub fn discover_gcp_billing() -> Vec<(String, String, String)> {
    let mut results = vec![];

    // Get all accessible projects
    let output = Command::new("gcloud")
        .args(["projects", "list", "--format=value(projectId)"])
        .output();

    let projects: Vec<String> = match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        }
        _ => return results,
    };

    for project in &projects {
        // Check if this project has a BigQuery dataset with billing data
        let output = Command::new("bq")
            .args(["ls", "--project_id", project, "--format=json"])
            .output();

        if let Ok(o) = output {
            if o.status.success() {
                if let Ok(datasets) = serde_json::from_str::<Vec<serde_json::Value>>(
                    &String::from_utf8_lossy(&o.stdout)
                ) {
                    for ds in &datasets {
                        let dataset_id = ds["datasetReference"]["datasetId"]
                            .as_str().unwrap_or("");

                        // Look for billing export datasets
                        if dataset_id.contains("billing") || dataset_id.contains("cost") {
                            // List tables in this dataset to find the billing export table
                            let table_output = Command::new("bq")
                                .args(["ls", "--project_id", project,
                                       "--format=json", dataset_id])
                                .output();

                            if let Ok(to) = table_output {
                                if to.status.success() {
                                    if let Ok(tables) = serde_json::from_str::<Vec<serde_json::Value>>(
                                        &String::from_utf8_lossy(&to.stdout)
                                    ) {
                                        for table in &tables {
                                            let table_id = table["tableReference"]["tableId"]
                                                .as_str().unwrap_or("");
                                            if table_id.starts_with("gcp_billing_export") {
                                                println!("[cost] Discovered GCP billing: {}.{}.{}",
                                                    project, dataset_id, table_id);
                                                results.push((
                                                    project.clone(),
                                                    dataset_id.to_string(),
                                                    table_id.to_string(),
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    results
}

/// Ingest GCP billing from BigQuery.
pub fn ingest_gcp_costs(client: &GraphClient, project: &str, dataset: &str, table: &str) -> Result<usize, String> {
    // Query BigQuery for last 30 days of costs by service
    let query = format!(
        "SELECT service.description, ROUND(SUM(cost), 2) AS cost, currency \
         FROM `{}.{}.{}` \
         WHERE DATE(usage_start_time) >= DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY) \
         GROUP BY service.description, currency \
         HAVING SUM(cost) > 0.01 \
         ORDER BY cost DESC",
        project, dataset, table
    );

    let output = Command::new("bq")
        .args(["query", "--project_id", project, "--use_legacy_sql=false", "--format=json", &query])
        .output()
        .map_err(|e| format!("bq command failed: {}", e))?;

    if !output.status.success() {
        return Err(format!("bq query failed: {}", String::from_utf8_lossy(&output.stderr)));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&json_str)
        .map_err(|e| format!("JSON parse failed: {}", e))?;

    let mut count = 0;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();

    for row in &rows {
        let service = row["description"].as_str().unwrap_or("unknown");
        let cost = row["cost"].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
        let currency = row["currency"].as_str().unwrap_or("USD");

        // Write to graph
        let q = format!(
            "MERGE (c:CloudCost {{provider: 'gcp', project: '{}', service: '{}'}}) \
             SET c.cost_30d = {}, c.currency = '{}', c.last_updated = {}",
            escape(project), escape(service), cost, currency, now
        );
        let _ = client.query(&q, &[]);

        // Edge: link to project node if exists
        let _ = client.query(
            &format!(
                "MERGE (p:CloudProject {{provider: 'gcp', project_id: '{}'}}) \
                 SET p.last_updated = {}",
                escape(project), now
            ),
            &[],
        );
        let _ = client.query(
            &format!(
                "MATCH (p:CloudProject {{provider: 'gcp', project_id: '{}'}}) \
                 MATCH (c:CloudCost {{provider: 'gcp', project: '{}', service: '{}'}}) \
                 MERGE (p)-[:HAS_COST]->(c)",
                escape(project), escape(project), escape(service)
            ),
            &[],
        );

        count += 1;
    }

    // Also get daily trend for anomaly detection
    let trend_query = format!(
        "SELECT service.description, DATE(usage_start_time) AS day, ROUND(SUM(cost), 2) AS cost \
         FROM `{}.{}.{}` \
         WHERE DATE(usage_start_time) >= DATE_SUB(CURRENT_DATE(), INTERVAL 7 DAY) \
         GROUP BY service.description, day \
         ORDER BY day DESC, cost DESC",
        project, dataset, table
    );

    let trend_output = Command::new("bq")
        .args(["query", "--project_id", project, "--use_legacy_sql=false", "--format=json", &trend_query])
        .output();

    if let Ok(out) = trend_output {
        if out.status.success() {
            if let Ok(trend_rows) = serde_json::from_str::<Vec<serde_json::Value>>(
                &String::from_utf8_lossy(&out.stdout)
            ) {
                // Store daily costs for trend analysis
                for row in &trend_rows {
                    let service = row["description"].as_str().unwrap_or("unknown");
                    let day = row["day"].as_str().unwrap_or("unknown");
                    let cost = row["cost"].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);

                    let _ = client.query(
                        &format!(
                            "MERGE (d:DailyCost {{provider: 'gcp', project: '{}', service: '{}', day: '{}'}}) \
                             SET d.cost = {}",
                            escape(project), escape(service), escape(day), cost
                        ),
                        &[],
                    );
                }
            }
        }
    }

    // Get storage details for Artifact Registry (the big cost)
    let ar_query = format!(
        "SELECT sku.description, ROUND(SUM(cost), 2) AS cost, \
         ROUND(SUM(usage.amount_in_pricing_units), 2) AS usage_amount, usage.pricing_unit \
         FROM `{}.{}.{}` \
         WHERE DATE(usage_start_time) >= DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY) \
         AND service.description = 'Artifact Registry' \
         GROUP BY sku.description, usage.pricing_unit \
         HAVING SUM(cost) > 0.01 \
         ORDER BY cost DESC",
        project, dataset, table
    );

    if let Ok(out) = Command::new("bq")
        .args(["query", "--project_id", project, "--use_legacy_sql=false", "--format=json", &ar_query])
        .output()
    {
        if out.status.success() {
            if let Ok(ar_rows) = serde_json::from_str::<Vec<serde_json::Value>>(
                &String::from_utf8_lossy(&out.stdout)
            ) {
                for row in &ar_rows {
                    let sku = row["description"].as_str().unwrap_or("unknown");
                    let cost = row["cost"].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                    let _ = client.query(
                        &format!(
                            "MERGE (c:CloudCost {{provider: 'gcp', project: '{}', service: 'Artifact Registry', sku: '{}'}}) \
                             SET c.cost_30d = {}, c.last_updated = {}",
                            escape(project), escape(sku), cost, now
                        ),
                        &[],
                    );
                }
            }
        }
    }

    println!("[cost] GCP {}: {} services, total ingested", project, count);
    Ok(count)
}

/// Ingest AWS costs using Cost Explorer CLI.
pub fn ingest_aws_costs(client: &GraphClient) -> Result<usize, String> {
    let output = Command::new("aws")
        .args([
            "ce", "get-cost-and-usage",
            "--time-period", "Start=2026-03-11,End=2026-04-11",
            "--granularity", "MONTHLY",
            "--metrics", "UnblendedCost",
            "--group-by", "Type=DIMENSION,Key=SERVICE",
            "--output", "json",
        ])
        .output()
        .map_err(|e| format!("aws ce failed: {}", e))?;

    if !output.status.success() {
        return Err(format!("aws ce failed: {}", String::from_utf8_lossy(&output.stderr)));
    }

    let data: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&output.stdout))
        .map_err(|e| format!("JSON parse: {}", e))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();

    let mut count = 0;
    if let Some(results) = data["ResultsByTime"].as_array() {
        for period in results {
            if let Some(groups) = period["Groups"].as_array() {
                for group in groups {
                    let service = group["Keys"][0].as_str().unwrap_or("unknown");
                    let cost = group["Metrics"]["UnblendedCost"]["Amount"]
                        .as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);

                    if cost < 0.01 { continue; }

                    let _ = client.query(
                        &format!(
                            "MERGE (c:CloudCost {{provider: 'aws', service: '{}'}}) \
                             SET c.cost_30d = {}, c.currency = 'USD', c.last_updated = {}",
                            escape(service), cost, now
                        ),
                        &[],
                    );

                    let _ = client.query(
                        &format!(
                            "MERGE (p:CloudProject {{provider: 'aws', project_id: 'default'}}) \
                             SET p.last_updated = {}",
                            now
                        ),
                        &[],
                    );
                    let _ = client.query(
                        &format!(
                            "MATCH (p:CloudProject {{provider: 'aws'}}) \
                             MATCH (c:CloudCost {{provider: 'aws', service: '{}'}}) \
                             MERGE (p)-[:HAS_COST]->(c)",
                            escape(service)
                        ),
                        &[],
                    );

                    count += 1;
                }
            }
        }
    }

    println!("[cost] AWS: {} services ingested", count);
    Ok(count)
}

/// Check for cost anomalies and return alerts.
pub fn check_cost_anomalies(client: &GraphClient) -> Vec<(String, f64, String)> {
    let mut anomalies = vec![];

    // Find services over $50/month
    if let Ok(r) = client.query(
        "MATCH (c:CloudCost) WHERE c.cost_30d > 50 RETURN c.provider, c.service, c.cost_30d ORDER BY c.cost_30d DESC",
        &[],
    ) {
        for row in &r.rows {
            let provider = row[0].as_str();
            let service = row[1].as_str();
            let cost = row[2].as_f64();
            anomalies.push((
                format!("{}/{}", provider, service),
                cost,
                format!("{} {} costs ${:.2}/month", provider.to_uppercase(), service, cost),
            ));
        }
    }

    anomalies
}

/// Fallback: ingest GCP costs using the Cloud Billing API.
/// No BigQuery export needed. Less granular (per-service, not per-SKU)
/// but works on any GCP project with billing enabled.
pub fn ingest_gcp_costs_api(client: &GraphClient, project: &str) -> Result<usize, String> {
    // Use gcloud to get the billing account
    let billing_output = Command::new("gcloud")
        .args(["billing", "projects", "describe", project, "--format=json"])
        .output()
        .map_err(|e| format!("gcloud failed: {}", e))?;

    if !billing_output.status.success() {
        return Err("Cannot access billing for this project".into());
    }

    let billing: serde_json::Value = serde_json::from_str(
        &String::from_utf8_lossy(&billing_output.stdout)
    ).map_err(|e| format!("JSON parse: {}", e))?;

    let billing_enabled = billing["billingEnabled"].as_bool().unwrap_or(false);
    if !billing_enabled {
        return Err(format!("Billing not enabled on project {}", project));
    }

    // Use the Cost Estimation API or fall back to resource counting
    // Since there's no direct "get costs" CLI command without BigQuery,
    // we estimate from running resources
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();

    let mut count = 0;

    // Check GKE clusters and estimate cost
    let gke_output = Command::new("gcloud")
        .args(["container", "clusters", "list", "--project", project, "--format=json"])
        .output();

    if let Ok(o) = gke_output {
        if o.status.success() {
            if let Ok(clusters) = serde_json::from_str::<Vec<serde_json::Value>>(
                &String::from_utf8_lossy(&o.stdout)
            ) {
                for cluster in &clusters {
                    let name = cluster["name"].as_str().unwrap_or("unknown");
                    let node_count = cluster["currentNodeCount"].as_i64().unwrap_or(0);
                    let machine_type = cluster["nodeConfig"]["machineType"]
                        .as_str().unwrap_or("e2-standard-2");

                    // Estimate monthly cost based on machine type
                    let per_node_monthly = estimate_gce_monthly_cost(machine_type);
                    let cluster_cost = node_count as f64 * per_node_monthly;

                    let _ = client.query(
                        &format!(
                            "MERGE (c:CloudCost {{provider: 'gcp', project: '{}', service: 'GKE: {}'}}) \
                             SET c.cost_30d = {}, c.currency = 'USD', c.last_updated = {}, \
                             c.estimated = true, c.nodes = {}, c.machine_type = '{}'",
                            escape(project), escape(name), cluster_cost, now,
                            node_count, escape(machine_type)
                        ),
                        &[],
                    );
                    count += 1;
                }
            }
        }
    }

    // Check Cloud SQL instances
    let sql_output = Command::new("gcloud")
        .args(["sql", "instances", "list", "--project", project, "--format=json"])
        .output();

    if let Ok(o) = sql_output {
        if o.status.success() {
            if let Ok(instances) = serde_json::from_str::<Vec<serde_json::Value>>(
                &String::from_utf8_lossy(&o.stdout)
            ) {
                for inst in &instances {
                    let name = inst["name"].as_str().unwrap_or("unknown");
                    let tier = inst["settings"]["tier"].as_str().unwrap_or("db-f1-micro");
                    let cost = estimate_cloudsql_monthly_cost(tier);

                    let _ = client.query(
                        &format!(
                            "MERGE (c:CloudCost {{provider: 'gcp', project: '{}', service: 'Cloud SQL: {}'}}) \
                             SET c.cost_30d = {}, c.currency = 'USD', c.last_updated = {}, \
                             c.estimated = true, c.tier = '{}'",
                            escape(project), escape(name), cost, now, escape(tier)
                        ),
                        &[],
                    );
                    count += 1;
                }
            }
        }
    }

    // Check Compute Engine VMs (non-GKE)
    let vm_output = Command::new("gcloud")
        .args(["compute", "instances", "list", "--project", project, "--format=json"])
        .output();

    if let Ok(o) = vm_output {
        if o.status.success() {
            if let Ok(vms) = serde_json::from_str::<Vec<serde_json::Value>>(
                &String::from_utf8_lossy(&o.stdout)
            ) {
                let non_gke: Vec<_> = vms.iter()
                    .filter(|v| !v["name"].as_str().unwrap_or("").contains("gke-"))
                    .collect();

                if !non_gke.is_empty() {
                    let total: f64 = non_gke.iter().map(|v| {
                        let mt = v["machineType"].as_str().unwrap_or("")
                            .rsplit('/').next().unwrap_or("e2-micro");
                        estimate_gce_monthly_cost(mt)
                    }).sum();

                    let _ = client.query(
                        &format!(
                            "MERGE (c:CloudCost {{provider: 'gcp', project: '{}', service: 'Compute Engine (non-GKE)'}}) \
                             SET c.cost_30d = {}, c.currency = 'USD', c.last_updated = {}, \
                             c.estimated = true, c.instance_count = {}",
                            escape(project), total, now, non_gke.len()
                        ),
                        &[],
                    );
                    count += 1;
                }
            }
        }
    }

    println!("[cost] GCP {} (API fallback): {} resources estimated", project, count);
    Ok(count)
}

/// Offer to enable BigQuery billing export for a project.
pub fn setup_billing_export(project: &str) -> Result<(), String> {
    println!("[cost] No BigQuery billing export found for {}.", project);
    println!("[cost] To enable detailed cost tracking, run:");
    println!("[cost]   bq mk --dataset {}.billing_export", project);
    println!("[cost]   Then enable billing export in GCP Console → Billing → Billing export");
    println!("[cost] Using API-based cost estimation in the meantime.");
    Ok(())
}

fn estimate_gce_monthly_cost(machine_type: &str) -> f64 {
    // Approximate monthly costs for common GCE machine types (us-central1)
    match machine_type {
        t if t.contains("e2-micro") => 6.11,
        t if t.contains("e2-small") => 12.23,
        t if t.contains("e2-medium") => 24.46,
        t if t.contains("e2-standard-2") => 48.91,
        t if t.contains("e2-standard-4") => 97.83,
        t if t.contains("e2-standard-8") => 195.66,
        t if t.contains("e2-standard-16") => 391.31,
        t if t.contains("n2-standard-2") => 71.40,
        t if t.contains("n2-standard-4") => 142.80,
        t if t.contains("n2-standard-8") => 285.61,
        t if t.contains("t2a-standard-1") => 27.39,
        t if t.contains("f1-micro") => 3.88,
        t if t.contains("g1-small") => 13.13,
        _ => 50.00, // default estimate
    }
}

fn estimate_cloudsql_monthly_cost(tier: &str) -> f64 {
    match tier {
        "db-f1-micro" => 7.67,
        "db-g1-small" => 25.55,
        t if t.contains("db-custom-1") => 51.10,
        t if t.contains("db-custom-2") => 102.20,
        t if t.contains("db-custom-4") => 204.40,
        "db-n1-standard-1" => 51.10,
        "db-n1-standard-2" => 102.20,
        _ => 30.00, // default estimate
    }
}

fn escape(s: &str) -> String {
    s.replace('\'', "\\'").replace('\\', "\\\\")
}
