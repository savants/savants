use clap::{Parser, Subcommand};
use colored::*;

mod graph;
mod commands;
mod utils;

pub use utils::find_in_path;

#[derive(Parser)]
#[command(name = "savants")]
#[command(about = "Your infrastructure savant. Know what's wrong in 60 seconds.")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Auto-detect infrastructure and show what's wrong
    Up {
        #[arg(long)]
        repo: Option<String>,
        #[arg(long, default_value = "200")]
        tail_lines: u32,
    },
    /// Full diagnosis narrative of your infrastructure
    Story {
        #[arg(long, default_value = "60")]
        since_minutes: u64,
        #[arg(long, default_value = "WARN")]
        min_severity: String,
        #[arg(long)]
        cluster: Option<String>,
        #[arg(long)]
        host: Option<String>,
    },
    /// Show graph stats and service health
    Status,
    /// Manage MCP server configuration
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Ingest a Kubernetes cluster
    K8s {
        #[command(subcommand)]
        action: K8sAction,
    },
    /// Ingest local host state
    Host {
        #[command(subcommand)]
        action: HostAction,
    },
    /// Start the MCP server (for Claude Code / Cursor)
    Serve,
}

#[derive(Subcommand)]
enum McpAction {
    /// One command to make Savants work with your AI assistant
    Install {
        #[arg(long, default_value = "project")]
        scope: String,
        #[arg(long, default_value = "auto")]
        tool: String,
    },
    /// Show current MCP configuration
    Status,
    /// Remove Savants MCP configuration
    Uninstall {
        #[arg(long, default_value = "all")]
        scope: String,
    },
}

#[derive(Subcommand)]
enum K8sAction {
    /// One-shot snapshot of a cluster
    Snapshot {
        cluster: String,
        #[arg(long)]
        context: Option<String>,
    },
    /// Live watch with log intelligence
    Watch {
        cluster: String,
        #[arg(long)]
        context: Option<String>,
        #[arg(long, default_value = "true")]
        logs: bool,
        #[arg(long, default_value = "0")]
        tail_lines: u32,
        #[arg(long, default_value = "24")]
        retention_hours: u32,
    },
}

#[derive(Subcommand)]
enum HostAction {
    /// One-shot host snapshot
    Snapshot,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Up { repo, tail_lines } => {
            commands::up::run(repo, tail_lines).await;
        }
        Commands::Story { since_minutes, min_severity, cluster, host } => {
            commands::story::run(since_minutes, &min_severity, cluster, host).await;
        }
        Commands::Status => {
            commands::status::run().await;
        }
        Commands::Mcp { action } => match action {
            McpAction::Install { scope, tool } => {
                commands::mcp::install(&scope, &tool);
            }
            McpAction::Status => {
                commands::mcp::status();
            }
            McpAction::Uninstall { scope } => {
                commands::mcp::uninstall(&scope);
            }
        },
        Commands::K8s { action } => match action {
            K8sAction::Snapshot { cluster, context } => {
                commands::agent::run_python(&["k8s", "snapshot", &cluster]);
            }
            K8sAction::Watch { cluster, context, logs, tail_lines, retention_hours } => {
                let mut args = vec!["k8s".to_string(), "watch".to_string(), cluster];
                if logs { args.push("--logs".into()); }
                args.extend(["--tail-lines".into(), tail_lines.to_string()]);
                args.extend(["--retention-hours".into(), retention_hours.to_string()]);
                let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                commands::agent::run_python(&refs);
            }
        },
        Commands::Host { action } => match action {
            HostAction::Snapshot => {
                commands::agent::run_python(&["host", "snapshot"]);
            }
        },
        Commands::Serve => {
            commands::agent::run_python_raw(&["-m", "savants.mcp"]);
        }
    }
}
