use clap::{Parser, Subcommand};
use colored::*;

mod alerts;
mod cloud;
mod cloud_cost;
mod remediation;
mod config;
mod embedded;
mod engine;
mod graph;
mod knowledge;
mod obfuscate;
mod saql;
mod schema;
mod commands;
mod host;
mod k8s;
mod mcp;
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
    /// Manage the Savants daemon (watches all infrastructure continuously)
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Manage remote agents (Tailscale auth key model)
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// Connect to external services (Slack, PagerDuty, savants.cloud)
    Connect {
        #[command(subcommand)]
        action: Option<ConnectAction>,
    },
    /// Disconnect from savants.cloud
    Disconnect,
    /// View internal state
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
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

#[derive(Subcommand)]
enum DaemonAction {
    /// Start the daemon in the background
    Start,
    /// Show what the daemon is watching
    Status,
    /// Stop the daemon
    Stop,
    /// Tail the daemon log
    Logs,
    /// Run the daemon in foreground (used internally)
    Run,
}

#[derive(Subcommand)]
enum AgentAction {
    /// Create a new agent key for a remote cluster
    Create {
        /// Cluster name this key is for (or --org-wide for any cluster)
        name: String,
        /// Allow this key to work for any cluster in the org
        #[arg(long)]
        org_wide: bool,
    },
    /// List all agent keys
    List,
    /// Revoke an agent key
    Revoke {
        /// Key prefix (first 12 chars) or name
        key_or_name: String,
    },
    /// Run as a headless agent (used inside K8s clusters)
    Run {
        /// Agent key (or set SAVANTS_AGENT_KEY env var)
        #[arg(long)]
        key: Option<String>,
        /// Cluster name to report as
        #[arg(long)]
        cluster: Option<String>,
    },
}

#[derive(Subcommand)]
enum ConnectAction {
    /// Connect Slack for alerts and interactive RCA
    Slack {
        /// Slack incoming webhook URL
        #[arg(long)]
        webhook: Option<String>,
        /// Slack bot token (for interactive mode)
        #[arg(long)]
        bot_token: Option<String>,
        /// Slack channel for alerts (required with --bot-token)
        #[arg(long)]
        channel: Option<String>,
    },
    /// Connect to savants.cloud for team federation
    Cloud,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Create or reset config to defaults
    Init,
    /// Show the config file path
    Path,
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
                run_k8s_snapshot(&cluster, context.as_deref()).await;
            }
            K8sAction::Watch { cluster, context, logs, tail_lines, retention_hours } => {
                run_k8s_watch(&cluster, context.as_deref()).await;
            }
        },
        Commands::Host { action } => match action {
            HostAction::Snapshot => {
                host::run_snapshot();
            }
        },
        Commands::Daemon { action } => match action {
            DaemonAction::Start => commands::daemon::start(),
            DaemonAction::Status => commands::daemon::status(),
            DaemonAction::Stop => commands::daemon::stop(),
            DaemonAction::Logs => commands::daemon::logs(),
            DaemonAction::Run => commands::daemon::run().await,
        },
        Commands::Serve => {
            let state = config::State::load();
            let graph_name = state.graph_name();
            match mcp::McpServer::new(&graph_name) {
                Ok(server) => server.run(),
                Err(e) => {
                    eprintln!("{}: failed to start MCP server: {}", "Error".red(), e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Agent { action } => match action {
            AgentAction::Create { name, org_wide } => {
                let scope = if org_wide { "org-wide" } else { &name };
                println!("{}", "Creating agent key...".bold());
                // Generate a secure random key
                use rand::Rng;
                let key_bytes: Vec<u8> = (0..32).map(|_| rand::thread_rng().gen::<u8>()).collect();
                let key_hex: String = key_bytes.iter().map(|b| format!("{:02x}", b)).collect();
                let full_key = format!("svt_agent_{}", key_hex);
                let prefix = &full_key[..20];

                println!();
                println!("Agent key for {}:", if org_wide { "all clusters".cyan() } else { name.cyan() });
                println!();
                println!("  {}", full_key.green().bold());
                println!();
                println!("{}", "Save this key — it won't be shown again.".yellow());
                println!();
                println!("Deploy to your cluster:");
                println!();
                println!("  {} {} \\", "kubectl create secret generic".dimmed(), "savants-agent".cyan());
                println!("    {} {}", "--from-literal=key=".dimmed(), full_key.dimmed());
                println!("  {} {}", "kubectl apply -f".dimmed(), "https://savants.dev/agent.yaml".cyan());
                println!();
                println!("Or with Helm:");
                println!();
                println!("  {} {} {} \\", "helm install".dimmed(), "savants-agent".cyan(), "savants/agent".dimmed());
                println!("    {} {} \\", "--set".dimmed(), format!("key={}", full_key).dimmed());
                println!("    {} {}", "--set".dimmed(), format!("cluster={}", name).dimmed());

                // TODO: When cloud is live, POST to /api/v1/org/agent-keys
                // to register the key hash server-side. For now, just print it.
            }
            AgentAction::List => {
                println!("{}", "Agent keys are managed in savants.cloud.".dimmed());
                println!("Run {} to connect first.", "savants connect".cyan());
            }
            AgentAction::Revoke { key_or_name } => {
                println!("{}", "Agent key revocation requires savants.cloud.".dimmed());
                println!("Run {} to connect first.", "savants connect".cyan());
            }
            AgentAction::Run { key, cluster } => {
                let agent_key = key
                    .or_else(|| std::env::var("SAVANTS_AGENT_KEY").ok())
                    .unwrap_or_else(|| {
                        eprintln!("{}: No agent key provided.", "Error".red());
                        eprintln!("Set --key or SAVANTS_AGENT_KEY env var.");
                        std::process::exit(1);
                    });
                let cluster_name = cluster
                    .or_else(|| std::env::var("SAVANTS_CLUSTER").ok())
                    .unwrap_or_else(|| {
                        // Try to detect from hostname
                        hostname::get()
                            .map(|h| h.to_string_lossy().to_string())
                            .unwrap_or_else(|_| "unknown".to_string())
                    });
                println!("{}", "Starting Savants agent...".bold());
                println!("  Cluster: {}", cluster_name.cyan());
                println!("  Key:     {}...", &agent_key[..std::cmp::min(20, agent_key.len())]);
                println!();
                // TODO: Run the ingest loop, pushing deltas to savants.cloud
                println!("{}", "Agent mode requires savants.cloud (coming soon).".yellow());
                println!("For local monitoring, use: {}", "savants k8s watch".cyan());
            }
        },
        Commands::Connect { action } => match action {
            Some(ConnectAction::Slack { webhook, bot_token, channel }) => {
                commands::connect::slack(webhook, bot_token, channel);
            }
            Some(ConnectAction::Cloud) | None => {
                commands::connect::run().await;
            }
        },
        Commands::Disconnect => {
            commands::connect::disconnect();
        }
        Commands::Config { action } => {
            match action {
                Some(ConfigAction::Init) => {
                    let state = config::State::default();
                    if let Err(e) = state.save() {
                        eprintln!("{}: {}", "Error".red(), e);
                    } else {
                        println!("State initialized.");
                    }
                }
                Some(ConfigAction::Path) => {
                    println!("{}", dirs::home_dir().unwrap_or_default().join(".savants").display());
                }
                None => {
                    let state = config::State::load();
                    println!("Graph:  {}:{}/{}", state.graph_host(), state.graph_port(), state.graph_name());
                    if state.is_cloud_authenticated() {
                        println!("Cloud:  {} (org: {})", "connected".green(), state.cloud_org.as_deref().unwrap_or("?"));
                    } else {
                        println!("Cloud:  {} (run {})", "not connected".dimmed(), "savants connect".cyan());
                    }
                }
            }
        }
    }
}

async fn run_k8s_snapshot(cluster: &str, context: Option<&str>) {
    println!("{}", format!("K8s snapshot: {}", cluster).bold());

    // Build kube client
    let kube_client = match k8s::K8sIngestor::kube_client_from_kubeconfig(context).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: failed to connect to cluster: {}", "Error".red(), e);
            return;
        }
    };

    // Build graph client using cluster-specific graph name
    let graph_name = config::State::cluster_graph_name(cluster);
    let graph = match graph::GraphClient::new(&graph_name) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{}: failed to connect to graph: {}", "Error".red(), e);
            return;
        }
    };

    let ingestor = k8s::K8sIngestor::new(graph, cluster.to_string(), kube_client);
    let stats = ingestor.snapshot().await;
    println!("{}", stats.summary());
}

async fn run_k8s_watch(cluster: &str, context: Option<&str>) {
    use std::sync::Arc;

    println!("{}", format!("K8s watch: {} (Ctrl+C to stop)", cluster).bold());

    let kube_client = match k8s::K8sIngestor::kube_client_from_kubeconfig(context).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: failed to connect to cluster: {}", "Error".red(), e);
            return;
        }
    };

    let graph_name = config::State::cluster_graph_name(cluster);
    let graph = match graph::GraphClient::new(&graph_name) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{}: failed to connect to graph: {}", "Error".red(), e);
            return;
        }
    };

    // Run initial snapshot first
    let ingestor = Arc::new(k8s::K8sIngestor::new(graph, cluster.to_string(), kube_client));
    println!("  Running initial snapshot...");
    let stats = ingestor.snapshot().await;
    println!("{}", stats.summary());
    println!();
    println!("  Starting watch streams...");

    let watcher = k8s::K8sWatcher::new(ingestor);
    let handle = watcher.start();

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await.ok();
    println!("\n  Stopping watch...");
    watcher.stop();
    handle.await.ok();
}
