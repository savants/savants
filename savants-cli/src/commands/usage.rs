use colored::*;
use crate::config::State;

const CLOUD_ENDPOINT: &str = "https://api.savants.cloud";

pub async fn run() {
    let state = State::load();

    let token = match state.cloud_device_token.as_deref() {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => {
            println!("{}", "Not connected to savants.cloud".yellow());
            println!("  Run {} to connect.", "savants connect".cyan());
            return;
        }
    };

    println!("{}", "Savants Usage".bold());
    println!();

    let client = reqwest::Client::new();
    let resp = match client.get(&format!("{}/api/v1/usage", CLOUD_ENDPOINT))
        .header("Authorization", format!("Bearer {}", token))
        .send().await
    {
        Ok(r) if r.status().is_success() => {
            r.json::<serde_json::Value>().await.unwrap_or_default()
        }
        Ok(r) if r.status().as_u16() == 401 => {
            println!("  {} Session expired. Run {} again.", "●".red(), "savants connect".cyan());
            return;
        }
        Ok(r) => {
            println!("  {} Cloud returned: {}", "●".red(), r.status());
            return;
        }
        Err(e) => {
            println!("  {} Could not reach savants.cloud: {}", "●".red(), e);
            return;
        }
    };

    let period = resp.get("period").and_then(|v| v.as_str()).unwrap_or("?");
    let total_calls = resp.get("total_calls").and_then(|v| v.as_i64()).unwrap_or(0);
    let total_cost_cents = resp.get("total_cost_cents").and_then(|v| v.as_i64()).unwrap_or(0);
    let plan = resp.get("plan").and_then(|v| v.as_str()).unwrap_or("?");
    let free_remaining = resp.get("free_remaining").and_then(|v| v.as_i64()).unwrap_or(-1);

    println!("  Period:     {}", period.cyan());
    println!("  Plan:       {}", plan.green());
    println!("  Total calls: {}", total_calls.to_string().bold());
    println!("  Total cost:  ${:.2}", total_cost_cents as f64 / 100.0);
    println!();

    if free_remaining >= 0 {
        if free_remaining > 0 {
            println!("  Free tier:   {} calls remaining", free_remaining.to_string().green());
        } else {
            println!("  {} Free tier exhausted.", "●".yellow());
            println!("    Upgrade to PAYG: {}", "https://savants.cloud/billing".cyan());
        }
        println!();
    }

    // Per-tool breakdown
    if let Some(tools) = resp.get("by_tool").and_then(|v| v.as_array()) {
        if !tools.is_empty() {
            println!("  {:<25} {:>6} {:>8} {:>10}", "Tool".dimmed(), "Calls".dimmed(), "Avg ms".dimmed(), "Cost".dimmed());
            println!("  {}", "-".repeat(55).dimmed());
            for tool in tools {
                let name = tool.get("tool").and_then(|v| v.as_str()).unwrap_or("?");
                let calls = tool.get("calls").and_then(|v| v.as_i64()).unwrap_or(0);
                let avg_ms = tool.get("avg_duration_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let cost_cents = tool.get("cost_cents").and_then(|v| v.as_i64()).unwrap_or(0);

                let cost_str = if cost_cents == 0 {
                    "free".green().to_string()
                } else {
                    format!("${:.2}", cost_cents as f64 / 100.0)
                };

                println!("  {:<25} {:>6} {:>7.0}ms {:>10}", name, calls, avg_ms, cost_str);
            }
        }
    }
}
