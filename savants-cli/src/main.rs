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
mod code_index;
mod code_parser;
mod code_graph;
mod semantic_search;
mod embeddings;
mod embedding_store;
mod github;
mod jira;
mod mcp;
mod radar;
mod sentry;
mod slack;
mod update_check;
mod utils;
mod kernel_probes;
mod ebpf_loader;
mod doc_parser;

pub use utils::find_in_path;

#[derive(Parser)]
#[command(name = "savants")]
#[command(about = "Your infrastructure savant. Know what's wrong in 60 seconds.")]
#[command(version = env!("SAVANTS_VERSION"))]
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
    /// Claude Code hook: intercepts grep/read when graph can answer better
    #[command(name = "hook")]
    Hook {
        #[command(subcommand)]
        action: HookAction,
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
    /// Show usage this month (calls, cost, by tool)
    Usage,
    /// Show time/token savings from savants vs native tools
    Stats {
        /// Number of days to look back (default: 7)
        #[arg(long, default_value = "7")]
        days: i64,
        /// Run live benchmark: savants vs grep/read side by side
        #[arg(long)]
        benchmark: bool,
    },
    /// Manage projects and their data sources
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
    /// Composable guardrails for AI coding agents
    Guard {
        /// Guard subcommand and arguments (e.g. list, preset standard, stats)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// View internal state
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },

    // ---- Direct tool shortcuts (no MCP client needed) ----

    /// Search code by natural language description
    Search {
        /// What to search for (e.g. "payment retry logic")
        query: String,
        /// Max results (default: 10)
        #[arg(long, short = 'n')]
        limit: Option<usize>,
    },
    /// Show file structure: functions, classes, types (no bodies)
    Skeleton {
        /// File path relative to repo root (e.g. "src/main.rs")
        file: String,
    },
    /// Find all callers of a function
    Callers {
        /// Function name to find callers of
        function: String,
    },
    /// Full structural profile of a function (params, callers, callees, source)
    Xray {
        /// Function name
        function: String,
        /// Optional file path to disambiguate
        #[arg(long, short = 'f')]
        file: Option<String>,
    },
    /// Show blast radius: what breaks if you change a function
    Blast {
        /// Function name
        function: String,
        /// Max traversal depth (default: 3)
        #[arg(long, short = 'd')]
        depth: Option<usize>,
    },
    /// Quick briefing: what changed since you last looked
    Brief {
        /// Time window (e.g. "2 hours ago", "1 day ago"). Defaults to since last brief.
        #[arg(long)]
        since: Option<String>,
    },
    /// Ask a question and get answers from indexed documentation
    Ask {
        /// The question to ask (e.g. "does FalkorDB support embedded mode?")
        question: String,
    },

    // ---- Graph algorithm commands ----

    /// Find circular dependencies in the codebase
    Cycles,
    /// Show top most-connected functions (PageRank)
    Hotspots {
        /// Number of top functions to show (default: 10)
        #[arg(long, short = 'n', default_value = "10")]
        top: usize,
    },
    /// Find shortest call path between two functions
    Path {
        /// Source function name
        from: String,
        /// Target function name
        to: String,
    },
    /// Show bridge/bottleneck functions (betweenness centrality)
    Bridges {
        /// Number of top functions to show (default: 10)
        #[arg(long, short = 'n', default_value = "10")]
        top: usize,
    },
    /// Show tightly-coupled function clusters
    Communities,
}

#[derive(Subcommand)]
enum ProjectAction {
    /// Create a new project
    Create {
        /// Project name (e.g. talent-pipeline)
        name: String,
        /// GitHub repo (e.g. sourcecoders-ai/talent-pipeline)
        #[arg(long)]
        github: Option<String>,
        /// Sentry project slug
        #[arg(long)]
        sentry: Option<String>,
        /// K8s namespace
        #[arg(long)]
        k8s: Option<String>,
    },
    /// List all projects
    List,
    /// Show project details with all sources
    Show {
        /// Project name or ID
        name: String,
    },
    /// Add a source to a project
    Connect {
        /// Project name
        project: String,
        /// Source type: github, sentry, k8s, slack
        source_type: String,
        /// Source identifier (repo name, project slug, namespace, channel)
        source_id: String,
    },
}

#[derive(Subcommand)]
enum HookAction {
    /// Intercept a tool call - called by Claude Code's PreToolUse hook
    Intercept,
    /// Post-tool context - called after Edit/Bash to suggest graph actions
    PostTool,
}

#[derive(Subcommand)]
enum McpAction {
    /// One command to make Savants work with your AI assistant
    Install {
        #[arg(long, default_value = "project")]
        scope: String,
        /// Target: auto, claude, cursor, windsurf, vscode, continue, zed
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
    /// Verify savants MCP is working (spawns server, calls tools, checks hooks)
    Test,
    /// Audit all configured MCP servers for security risks
    Audit,
    /// Show MCP tool call statistics and anomaly detection
    McpStats {
        /// Number of hours to look back (default: 24)
        #[arg(long, default_value = "24")]
        hours: u64,
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
    /// Install and start the agent as a background daemon
    Start,
    /// Stop the agent daemon
    Stop,
    /// Show agent daemon status
    Status,
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
    /// Run in foreground (used inside K8s clusters or for debugging)
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
        /// Personal user token from browser (xoxc-*)
        #[arg(long)]
        user_token: Option<String>,
        /// Browser cookie 'd' value (required with --user-token)
        #[arg(long)]
        cookie: Option<String>,
        /// Slack channel for alerts (required with --bot-token or --user-token)
        #[arg(long)]
        channel: Option<String>,
        /// Auto-extract token from browser (starts local server, paste one-liner in Slack console)
        #[arg(long)]
        from_browser: bool,
    },
    /// Connect Sentry for error tracking
    Sentry {
        /// Sentry auth token
        #[arg(long)]
        token: Option<String>,
        /// Sentry organization slug
        #[arg(long)]
        org: Option<String>,
    },
    /// Connect Jira for ticket tracking
    Jira {
        /// Jira instance URL (e.g. https://yourcompany.atlassian.net)
        #[arg(long)]
        url: Option<String>,
        /// Jira user email
        #[arg(long)]
        user: Option<String>,
        /// Jira API token
        #[arg(long)]
        token: Option<String>,
        /// Jira project key (e.g. VSCV)
        #[arg(long)]
        project: Option<String>,
    },
    /// Connect GitHub for PR tracking
    Github {
        /// GitHub repo in owner/repo format
        #[arg(long)]
        repo: Option<String>,
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
    /// Manage anonymous telemetry (on/off/status)
    Telemetry {
        /// Action: on, off, or status
        action: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Background update check (non-blocking, cached 24h)
    update_check::check_background();

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
        Commands::Hook { action } => match action {
            HookAction::Intercept => {
                commands::hooks::intercept();
            }
            HookAction::PostTool => {
                commands::hooks::post_tool();
            }
        },
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
            McpAction::Test => {
                commands::mcp::test();
            }
            McpAction::Audit => {
                commands::mcp::audit();
            }
            McpAction::McpStats { hours } => {
                commands::mcp::mcp_stats(hours);
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
            // Default: cloud mode (D1 + Vectorize). All graph data lives in the cloud.
            // Only use local FalkorDB if SAVANTS_LOCAL=1 is explicitly set.
            let use_local = std::env::var("SAVANTS_LOCAL").unwrap_or_default() == "1";

            if use_local {
                // Local mode: connect directly to FalkorDB (dev/testing only)
                let state = config::State::load();
                let graph_name = state.graph_name();
                match mcp::McpServer::new(&graph_name) {
                    Ok(server) => server.run(),
                    Err(e) => {
                        eprintln!("{}: failed to start MCP server: {}", "Error".red(), e);
                        std::process::exit(1);
                    }
                }
            } else {
                // Cloud mode: proxy all tool calls to api.savants.cloud
                let cloud_url = std::env::var("SAVANTS_CLOUD_URL")
                    .unwrap_or_else(|_| "https://api.savants.cloud".to_string());
                let api_key = std::env::var("SAVANTS_API_KEY").unwrap_or_default();

                // Try to load API key from config if not in env
                let api_key = if api_key.is_empty() {
                    let state = config::State::load();
                    state.cloud_token().unwrap_or_default()
                } else {
                    api_key
                };

                if api_key.is_empty() {
                    eprintln!("{}: No API key found. Run 'savants connect cloud' first.", "Error".red());
                    std::process::exit(1);
                }

                let proxy = mcp::CloudProxyServer::new(&cloud_url, &api_key);
                proxy.run();
            }
        }
        Commands::Agent { action } => match action {
            AgentAction::Start => {
                commands::agent::daemon_start().await;
            }
            AgentAction::Stop => {
                commands::agent::daemon_stop();
            }
            AgentAction::Status => {
                commands::agent::daemon_status();
            }
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
            AgentAction::Run { key: _, cluster } => {
                commands::agent::start(cluster).await;
            }
        },
        Commands::Connect { action } => match action {
            Some(ConnectAction::Slack { webhook, bot_token, user_token, cookie, channel, from_browser }) => {
                if from_browser {
                    commands::connect::slack_from_browser().await;
                } else {
                    commands::connect::slack(webhook, bot_token, user_token, cookie, channel);
                }
            }
            Some(ConnectAction::Sentry { token, org }) => {
                commands::connect::sentry(token, org);
            }
            Some(ConnectAction::Jira { url, user, token, project }) => {
                commands::connect::jira(url, user, token, project);
            }
            Some(ConnectAction::Github { repo }) => {
                commands::connect::github(repo);
            }
            Some(ConnectAction::Cloud) | None => {
                commands::connect::run().await;
            }
        },
        Commands::Disconnect => {
            commands::connect::disconnect();
        }
        Commands::Usage => {
            commands::usage::run().await;
        }
        Commands::Stats { days, benchmark } => {
            if benchmark {
                commands::stats::benchmark();
            } else {
                commands::stats::run(days);
            }
        }
        Commands::Project { action } => {
            commands::project::run(action).await;
        }
        Commands::Guard { args } => {
            commands::guard::run(args);
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
                Some(ConfigAction::Telemetry { action }) => {
                    let state_path = dirs::home_dir()
                        .unwrap_or_default()
                        .join(".savants")
                        .join("state.json");

                    let action_str = action.as_deref().unwrap_or("status");

                    match action_str {
                        "off" | "disable" => {
                            let mut state: serde_json::Value = std::fs::read_to_string(&state_path)
                                .ok()
                                .and_then(|s| serde_json::from_str(&s).ok())
                                .unwrap_or_else(|| serde_json::json!({}));

                            state.as_object_mut().unwrap()
                                .insert("telemetry_enabled".to_string(), serde_json::json!(false));

                            if let Some(parent) = state_path.parent() {
                                std::fs::create_dir_all(parent).ok();
                            }
                            std::fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap_or_default()).ok();

                            // Remove last heartbeat file so it doesn't send stale data
                            let last_path = dirs::home_dir().unwrap_or_default()
                                .join(".savants").join("telemetry-last.txt");
                            std::fs::remove_file(&last_path).ok();

                            println!("Telemetry {}", "disabled".yellow());
                            println!("  No anonymous usage data will be sent.");
                            println!("  You can also set {} or {}", "DO_NOT_TRACK=1".cyan(), "SAVANTS_DO_NOT_TRACK=1".cyan());
                        }
                        "on" | "enable" => {
                            let mut state: serde_json::Value = std::fs::read_to_string(&state_path)
                                .ok()
                                .and_then(|s| serde_json::from_str(&s).ok())
                                .unwrap_or_else(|| serde_json::json!({}));

                            state.as_object_mut().unwrap()
                                .insert("telemetry_enabled".to_string(), serde_json::json!(true));

                            // Generate telemetry_id if missing
                            if state.get("telemetry_id").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
                                use rand::Rng;
                                let id_bytes: Vec<u8> = (0..8).map(|_| rand::thread_rng().gen::<u8>()).collect();
                                let id_hex: String = id_bytes.iter().map(|b| format!("{:02x}", b)).collect();
                                state.as_object_mut().unwrap()
                                    .insert("telemetry_id".to_string(), serde_json::json!(format!("sv_{}", id_hex)));
                            }

                            if let Some(parent) = state_path.parent() {
                                std::fs::create_dir_all(parent).ok();
                            }
                            std::fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap_or_default()).ok();

                            println!("Telemetry {}", "enabled".green());
                            println!("  Anonymous usage data helps improve savants.");
                            println!("  No code, file paths, or command arguments are ever sent.");
                        }
                        _ => {
                            // Status
                            let state: serde_json::Value = std::fs::read_to_string(&state_path)
                                .ok()
                                .and_then(|s| serde_json::from_str(&s).ok())
                                .unwrap_or_else(|| serde_json::json!({}));

                            let enabled = state.get("telemetry_enabled")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true);
                            let telemetry_id = state.get("telemetry_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("(not set)");

                            let env_disabled = std::env::var("DO_NOT_TRACK").unwrap_or_default() == "1"
                                || std::env::var("SAVANTS_DO_NOT_TRACK").unwrap_or_default() == "1";

                            println!("Telemetry status:");
                            if env_disabled {
                                println!("  State: {} (env var override)", "disabled".yellow());
                            } else if enabled {
                                println!("  State: {}", "enabled".green());
                            } else {
                                println!("  State: {}", "disabled".yellow());
                            }
                            println!("  ID:    {}", telemetry_id.dimmed());
                            println!();
                            println!("What's collected (privacy-safe only):");
                            println!("  Tool name (e.g. Bash), rule count, preset name,");
                            println!("  OS, arch, CLI version, hashed hostname.");
                            println!();
                            println!("What's NEVER collected:");
                            println!("  Command arguments, file paths, code, rule content,");
                            println!("  email, username, or any PII.");
                            println!();
                            println!("Manage:");
                            println!("  {} {}", "savants config telemetry off".cyan(), "# disable".dimmed());
                            println!("  {} {}", "savants config telemetry on ".cyan(), "# enable".dimmed());
                        }
                    }
                }
                None => {
                    let state = config::State::load();
                    println!("Context engine: {}:{}", state.graph_host(), state.graph_port());
                    if state.is_cloud_authenticated() {
                        println!("Cloud:  {} (org: {})", "connected".green(), state.cloud_org.as_deref().unwrap_or("?"));
                    } else {
                        println!("Cloud:  {} (run {})", "not connected".dimmed(), "savants connect".cyan());
                    }
                }
            }
        }

        // Direct tool shortcuts
        Commands::Search { query, limit } => {
            commands::tools::search(&query, limit);
        }
        Commands::Skeleton { file } => {
            commands::tools::skeleton(&file);
        }
        Commands::Callers { function } => {
            commands::tools::callers(&function);
        }
        Commands::Xray { function, file } => {
            commands::tools::xray(&function, file.as_deref());
        }
        Commands::Blast { function, depth } => {
            commands::tools::blast(&function, depth);
        }
        Commands::Brief { since } => {
            commands::tools::brief(since.as_deref());
        }
        Commands::Ask { question } => {
            commands::tools::ask(&question);
        }

        // Graph algorithm commands
        Commands::Cycles => {
            let repo = commands::tools::detect_repo_name_pub();
            match code_graph::load_graph(&repo) {
                Ok(g) => println!("{}", g.find_cycles()),
                Err(e) => eprintln!("{}: {}", "Error".red(), e),
            }
        }
        Commands::Hotspots { top } => {
            let repo = commands::tools::detect_repo_name_pub();
            match code_graph::load_graph(&repo) {
                Ok(g) => println!("{}", g.page_rank(top)),
                Err(e) => eprintln!("{}: {}", "Error".red(), e),
            }
        }
        Commands::Path { from, to } => {
            let repo = commands::tools::detect_repo_name_pub();
            match code_graph::load_graph(&repo) {
                Ok(g) => println!("{}", g.shortest_path(&from, &to)),
                Err(e) => eprintln!("{}: {}", "Error".red(), e),
            }
        }
        Commands::Bridges { top } => {
            let repo = commands::tools::detect_repo_name_pub();
            match code_graph::load_graph(&repo) {
                Ok(g) => println!("{}", g.bridge_nodes(top)),
                Err(e) => eprintln!("{}: {}", "Error".red(), e),
            }
        }
        Commands::Communities => {
            let repo = commands::tools::detect_repo_name_pub();
            match code_graph::load_graph(&repo) {
                Ok(g) => println!("{}", g.communities()),
                Err(e) => eprintln!("{}: {}", "Error".red(), e),
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
            eprintln!("{}: failed to connect to context engine: {}", "Error".red(), e);
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
            eprintln!("{}: failed to connect to context engine: {}", "Error".red(), e);
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

    // Post initial snapshot summary to cloud
    if let Some(ref sink) = watcher.cloud_sink {
        sink.record("snapshot", "Cluster", cluster, "", "applied", "info",
            &format!("ns={} deploy={} pods={} svc={} cm={} secrets={}",
                stats.namespaces.added, stats.deployments.added,
                stats.pods.added, stats.services.added,
                stats.configmaps.added, stats.secrets.added));
        sink.flush();
        eprintln!("[k8s-cloud] Initial snapshot pushed to cloud");
    }

    let handle = watcher.start();

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await.ok();
    println!("\n  Stopping watch...");

    // Flush remaining cloud events before shutdown
    if let Some(ref sink) = watcher.cloud_sink {
        sink.flush();
    }

    watcher.stop();
    handle.await.ok();
}
