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

fn escape(s: &str) -> String {
    s.replace('\'', "\\'").replace('\\', "\\\\")
}
