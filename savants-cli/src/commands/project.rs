//! Project management: create, list, connect sources.
//!
//! Wraps the /api/v1/projects cloud API.
//!
//! Usage:
//!   savants project create talent-pipeline --github sourcecoders-ai/talent-pipeline --sentry vocator-backend
//!   savants project list
//!   savants project show talent-pipeline
//!   savants project connect talent-pipeline github sourcecoders-ai/talent-pipeline

use colored::*;

use crate::ProjectAction;

const CLOUD_URL: &str = "https://api.savants.cloud";

pub async fn run(action: ProjectAction) {
    let state = crate::config::State::load();
    let token = match state.cloud_token() {
        Some(t) => t,
        None => {
            eprintln!("{}: Not connected. Run: savants connect", "Error".red());
            return;
        }
    };

    let client = reqwest::Client::new();

    match action {
        ProjectAction::Create {
            name,
            github,
            sentry,
            k8s,
        } => {
            println!("{}", "Creating project...".bold());

            // Create project
            let res = client
                .post(format!("{}/api/v1/projects", CLOUD_URL))
                .header("Authorization", format!("Bearer {}", token))
                .json(&serde_json::json!({
                    "name": name,
                    "description": format!("Created via CLI"),
                }))
                .send()
                .await;

            let project_id = match res {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    let id = body["id"].as_str().unwrap_or("").to_string();
                    println!(
                        "  {} Project '{}' created ({})",
                        ">>".green(),
                        name.cyan(),
                        &id[..8]
                    );
                    id
                }
                Ok(resp) => {
                    let text = resp.text().await.unwrap_or_default();
                    eprintln!("{}: {}", "Error".red(), text);
                    return;
                }
                Err(e) => {
                    eprintln!("{}: {}", "Error".red(), e);
                    return;
                }
            };

            // Add GitHub source
            if let Some(repo) = github {
                let parts: Vec<&str> = repo.split('/').collect();
                let (owner, repo_name) = if parts.len() == 2 {
                    (parts[0], parts[1])
                } else {
                    ("", repo.as_str())
                };

                let res = client
                    .post(format!("{}/api/v1/projects/{}/sources", CLOUD_URL, project_id))
                    .header("Authorization", format!("Bearer {}", token))
                    .json(&serde_json::json!({
                        "source_type": "github_repo",
                        "config": {"owner": owner, "repo": repo_name, "full_name": repo},
                    }))
                    .send()
                    .await;

                match res {
                    Ok(r) if r.status().is_success() => {
                        println!("  {} GitHub: {}", ">>".green(), repo.cyan());
                    }
                    _ => eprintln!("  {} GitHub connection failed", "!!".yellow()),
                }
            }

            // Add Sentry source
            if let Some(project_slug) = sentry {
                let res = client
                    .post(format!("{}/api/v1/projects/{}/sources", CLOUD_URL, project_id))
                    .header("Authorization", format!("Bearer {}", token))
                    .json(&serde_json::json!({
                        "source_type": "sentry_project",
                        "config": {"project_slug": project_slug},
                    }))
                    .send()
                    .await;

                match res {
                    Ok(r) if r.status().is_success() => {
                        println!("  {} Sentry: {}", ">>".green(), project_slug.cyan());
                    }
                    _ => eprintln!("  {} Sentry connection failed", "!!".yellow()),
                }
            }

            // Add K8s source
            if let Some(namespace) = k8s {
                let res = client
                    .post(format!("{}/api/v1/projects/{}/sources", CLOUD_URL, project_id))
                    .header("Authorization", format!("Bearer {}", token))
                    .json(&serde_json::json!({
                        "source_type": "k8s_namespace",
                        "config": {"namespace": namespace},
                    }))
                    .send()
                    .await;

                match res {
                    Ok(r) if r.status().is_success() => {
                        println!("  {} K8s: {}", ">>".green(), namespace.cyan());
                    }
                    _ => eprintln!("  {} K8s connection failed", "!!".yellow()),
                }
            }

            println!("\n{}", "Done. Run 'savants project show' to see status.".dimmed());
        }

        ProjectAction::List => {
            let res = client
                .get(format!("{}/api/v1/projects", CLOUD_URL))
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await;

            match res {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    let projects = body["projects"].as_array().cloned().unwrap_or_default();

                    if projects.is_empty() {
                        println!("No projects. Create one: savants project create <name>");
                        return;
                    }

                    println!("{}", "Projects:".bold());
                    for p in &projects {
                        let name = p["name"].as_str().unwrap_or("?");
                        let sources = p["source_count"].as_u64().unwrap_or(0);
                        let members = p["member_count"].as_u64().unwrap_or(0);
                        println!(
                            "  {} {} sources, {} members",
                            name.cyan(),
                            sources,
                            members
                        );
                    }
                }
                _ => eprintln!("{}: Failed to fetch projects", "Error".red()),
            }
        }

        ProjectAction::Show { name } => {
            // First list projects to find by name
            let res = client
                .get(format!("{}/api/v1/projects", CLOUD_URL))
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await;

            let project_id = match res {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    let projects = body["projects"].as_array().cloned().unwrap_or_default();
                    projects
                        .iter()
                        .find(|p| {
                            p["slug"].as_str() == Some(&name)
                                || p["name"].as_str() == Some(&name)
                                || p["id"].as_str() == Some(&name)
                        })
                        .and_then(|p| p["id"].as_str().map(|s| s.to_string()))
                }
                _ => None,
            };

            let project_id = match project_id {
                Some(id) => id,
                None => {
                    eprintln!("Project '{}' not found", name);
                    return;
                }
            };

            let res = client
                .get(format!("{}/api/v1/projects/{}", CLOUD_URL, project_id))
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await;

            match res {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    let project = &body["project"];
                    let sources = body["sources"].as_array().cloned().unwrap_or_default();
                    let members = body["members"].as_array().cloned().unwrap_or_default();

                    println!("{} {}", "Project:".bold(), project["name"].as_str().unwrap_or("?").cyan());
                    println!("  ID: {}", project["id"].as_str().unwrap_or("?"));
                    println!("  Slug: {}", project["slug"].as_str().unwrap_or("?"));

                    println!("\n{} ({})", "Sources:".bold(), sources.len());
                    for s in &sources {
                        let stype = s["source_type"].as_str().unwrap_or("?");
                        let enabled = s["enabled"].as_i64().unwrap_or(0) == 1;
                        let status = if enabled { "active".green() } else { "disabled".red() };
                        let config = &s["config"];
                        let detail = match stype {
                            "github_repo" => config["full_name"].as_str().unwrap_or("?").to_string(),
                            "sentry_project" => config["project_slug"].as_str().unwrap_or("?").to_string(),
                            "k8s_namespace" => config["namespace"].as_str().unwrap_or("?").to_string(),
                            "slack_channel" => config["channel_name"].as_str().unwrap_or("?").to_string(),
                            _ => format!("{}", config),
                        };
                        println!("  {} {} [{}] - {}", ">>".dimmed(), stype, status, detail);
                    }

                    println!("\n{} ({})", "Members:".bold(), members.len());
                    for m in &members {
                        let email = m["email"].as_str().unwrap_or("?");
                        let role = m["role"].as_str().unwrap_or("member");
                        println!("  {} ({}) - {}", email, role, m["name"].as_str().unwrap_or(""));
                    }
                }
                _ => eprintln!("{}: Failed to fetch project", "Error".red()),
            }
        }

        ProjectAction::Connect {
            project,
            source_type,
            source_id,
        } => {
            // Find project ID
            let res = client
                .get(format!("{}/api/v1/projects", CLOUD_URL))
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await;

            let project_id = match res {
                Ok(resp) if resp.status().is_success() => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    body["projects"]
                        .as_array()
                        .and_then(|ps| {
                            ps.iter().find(|p| {
                                p["slug"].as_str() == Some(&project)
                                    || p["name"].as_str() == Some(&project)
                            })
                        })
                        .and_then(|p| p["id"].as_str().map(|s| s.to_string()))
                }
                _ => None,
            };

            let project_id = match project_id {
                Some(id) => id,
                None => {
                    eprintln!("Project '{}' not found", project);
                    return;
                }
            };

            let (stype, config) = match source_type.as_str() {
                "github" => {
                    let parts: Vec<&str> = source_id.split('/').collect();
                    let (owner, repo) = if parts.len() == 2 {
                        (parts[0], parts[1])
                    } else {
                        ("", source_id.as_str())
                    };
                    (
                        "github_repo",
                        serde_json::json!({"owner": owner, "repo": repo, "full_name": source_id}),
                    )
                }
                "sentry" => (
                    "sentry_project",
                    serde_json::json!({"project_slug": source_id}),
                ),
                "k8s" => (
                    "k8s_namespace",
                    serde_json::json!({"namespace": source_id}),
                ),
                "slack" => (
                    "slack_channel",
                    serde_json::json!({"channel_name": source_id}),
                ),
                other => {
                    eprintln!("Unknown source type: {}. Use: github, sentry, k8s, slack", other);
                    return;
                }
            };

            let res = client
                .post(format!(
                    "{}/api/v1/projects/{}/sources",
                    CLOUD_URL, project_id
                ))
                .header("Authorization", format!("Bearer {}", token))
                .json(&serde_json::json!({
                    "source_type": stype,
                    "config": config,
                }))
                .send()
                .await;

            match res {
                Ok(r) if r.status().is_success() => {
                    println!(
                        "{} Connected {} to {}",
                        ">>".green(),
                        source_id.cyan(),
                        project.cyan()
                    );
                }
                Ok(r) => {
                    let text = r.text().await.unwrap_or_default();
                    eprintln!("{}: {}", "Error".red(), text);
                }
                Err(e) => eprintln!("{}: {}", "Error".red(), e),
            }
        }
    }
}
