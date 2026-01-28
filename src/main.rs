mod api;
mod config;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "sentry")]
#[command(about = "CLI tool to access Sentry API")]
struct Cli {
    /// Site to use (e.g., mh, gc). Defaults to default_site in config.
    #[arg(short, long, global = true)]
    site: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Configure organization and auth token
    Config {
        /// Sentry organization slug (e.g., globalcomix)
        #[arg(short, long)]
        org: Option<String>,
        /// Auth token (from sentry.io/settings/auth-tokens/)
        #[arg(short, long)]
        token: Option<String>,
        /// Set this site as the default
        #[arg(long)]
        default: bool,
        /// List configured sites
        #[arg(short, long)]
        list: bool,
    },
    /// Get issue details
    Issue {
        /// Issue ID
        id: String,
        #[command(subcommand)]
        command: Option<IssueCommands>,
    },
    /// List projects in the organization
    Projects,
    /// List issues for a project
    Issues {
        /// Project slug
        project: String,
        /// Search query (default: is:unresolved)
        #[arg(short, long)]
        query: Option<String>,
    },
    /// List cron monitors
    Monitors {
        /// Filter by environment
        #[arg(short, long)]
        environment: Option<String>,
    },
    /// Get cron monitor details
    Monitor {
        /// Monitor slug
        slug: String,
        #[command(subcommand)]
        command: Option<MonitorCommands>,
    },
    /// [Redirect] Get events for an issue
    #[command(hide = true)]
    Events {
        /// Issue ID
        id: Option<String>,
    },
    /// List internal integrations
    Integrations,
    /// Get integration details
    Integration {
        /// Integration slug (e.g., claude-agent-4744fc)
        slug: String,
    },
}

#[derive(Subcommand)]
enum IssueCommands {
    /// Get the latest event for this issue
    Latest,
    /// Get all events for this issue
    Events,
    /// Get a specific event by ID
    Event {
        /// Event ID
        event_id: String,
    },
    /// Get hashes for this issue
    Hashes,
    /// Mark issue as resolved
    Resolve,
}

#[derive(Subcommand)]
enum MonitorCommands {
    /// List recent check-ins for this monitor
    Checkins {
        /// Number of check-ins to fetch (default: 20)
        #[arg(short, long)]
        limit: Option<u32>,
    },
}


fn get_client(site: Option<&str>) -> Result<api::Client> {
    let cfg = config::load_config()?;
    let site_cfg = cfg.get_site(site);

    let org = site_cfg.organization.ok_or_else(|| {
        if let Some(s) = site {
            anyhow::anyhow!(
                "Organization not configured for site '{}'. Run 'sentry config -s {} -o <org>' first",
                s, s
            )
        } else {
            anyhow::anyhow!("Organization not configured. Run 'sentry config -o <org>' first")
        }
    })?;

    let token = site_cfg.auth_token.ok_or_else(|| {
        if let Some(s) = site {
            anyhow::anyhow!(
                "Auth token not configured for site '{}'. Run 'sentry config -s {} -t <token>' first",
                s, s
            )
        } else {
            anyhow::anyhow!("Auth token not configured. Run 'sentry config -t <token>' first")
        }
    })?;

    api::Client::new(&org, &token)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let site = cli.site.as_deref();

    match cli.command {
        Commands::Config {
            org,
            token,
            default,
            list,
        } => {
            let mut cfg = config::load_config().unwrap_or_default();

            if list {
                let sites = cfg.list_sites();
                if sites.is_empty() {
                    println!("No sites configured.");
                    if cfg.organization.is_some() {
                        println!("Legacy config found (org: {})", cfg.organization.as_deref().unwrap_or("?"));
                    }
                } else {
                    println!("Configured sites:");
                    for s in sites {
                        let marker = if cfg.default_site.as_deref() == Some(s) {
                            " (default)"
                        } else {
                            ""
                        };
                        let site_cfg = cfg.sites.get(s).unwrap();
                        println!(
                            "  {}{}: org={}",
                            s,
                            marker,
                            site_cfg.organization.as_deref().unwrap_or("?")
                        );
                    }
                }
                return Ok(());
            }

            if let Some(s) = site {
                // Configure a specific site
                cfg.set_site(s, org, token);
                if default {
                    cfg.default_site = Some(s.to_string());
                }
                config::save_config(&cfg)?;
                println!("Config saved for site '{}'", s);
            } else if org.is_some() || token.is_some() {
                // Legacy mode: set top-level config
                if let Some(o) = org {
                    cfg.organization = Some(o);
                }
                if let Some(t) = token {
                    cfg.auth_token = Some(t);
                }
                config::save_config(&cfg)?;
                println!("Config saved to ~/.config/sentry-cli-rs/config.json");
            } else {
                // Show current config
                if let Some(s) = &cfg.default_site {
                    println!("Default site: {}", s);
                }
                let sites = cfg.list_sites();
                if !sites.is_empty() {
                    println!("Sites: {}", sites.join(", "));
                }
                if cfg.organization.is_some() {
                    println!("Legacy org: {}", cfg.organization.as_deref().unwrap_or("?"));
                }
            }
        }
        Commands::Issue { id, command } => {
            let client = get_client(site)?;
            match command {
                None => {
                    let issue = client.get_issue(&id).await?;
                    println!("{}", serde_json::to_string_pretty(&issue)?);
                }
                Some(IssueCommands::Latest) => {
                    let event = client.get_issue_latest_event(&id).await?;
                    println!("{}", serde_json::to_string_pretty(&event)?);
                }
                Some(IssueCommands::Events) => {
                    let events = client.get_issue_events(&id).await?;
                    println!("{}", serde_json::to_string_pretty(&events)?);
                }
                Some(IssueCommands::Event { event_id }) => {
                    let event = client.get_issue_event(&id, &event_id).await?;
                    println!("{}", serde_json::to_string_pretty(&event)?);
                }
                Some(IssueCommands::Hashes) => {
                    let hashes = client.get_issue_hashes(&id).await?;
                    println!("{}", serde_json::to_string_pretty(&hashes)?);
                }
                Some(IssueCommands::Resolve) => {
                    let result = client.resolve_issue(&id).await?;
                    let short_id = result["shortId"].as_str().unwrap_or(&id);
                    println!("Resolved {}", short_id);
                }
            }
        }
        Commands::Projects => {
            let client = get_client(site)?;
            let projects = client.list_projects().await?;
            println!("{}", serde_json::to_string_pretty(&projects)?);
        }
        Commands::Issues { project, query } => {
            let client = get_client(site)?;
            let issues = client.list_issues(&project, query.as_deref()).await?;
            println!("{}", serde_json::to_string_pretty(&issues)?);
        }
        Commands::Monitors { environment } => {
            let client = get_client(site)?;
            let monitors = client.list_monitors(environment.as_deref()).await?;
            println!("{}", serde_json::to_string_pretty(&monitors)?);
        }
        Commands::Monitor { slug, command } => {
            let client = get_client(site)?;
            match command {
                None => {
                    let monitor = client.get_monitor(&slug).await?;
                    println!("{}", serde_json::to_string_pretty(&monitor)?);
                }
                Some(MonitorCommands::Checkins { limit }) => {
                    let checkins = client.list_monitor_checkins(&slug, limit).await?;
                    println!("{}", serde_json::to_string_pretty(&checkins)?);
                }
            }
        }
        Commands::Events { id } => {
            eprintln!("Error: 'events' is a subcommand of 'issue', not a top-level command.");
            eprintln!();
            if let Some(issue_id) = id {
                eprintln!("Usage: sentry issue {} events", issue_id);
            } else {
                eprintln!("Usage: sentry issue <ISSUE_ID> events");
            }
            std::process::exit(1);
        }
        Commands::Integrations => {
            let client = get_client(site)?;
            let integrations = client.list_integrations().await?;
            if let Some(arr) = integrations.as_array() {
                if arr.is_empty() {
                    println!("No internal integrations found.");
                } else {
                    for int in arr {
                        let name = int["name"].as_str().unwrap_or("?");
                        let slug = int["slug"].as_str().unwrap_or("?");
                        let webhook = int["webhookUrl"].as_str().unwrap_or("-");
                        println!("{} ({})", name, slug);
                        if webhook != "-" {
                            println!("  webhook: {}", webhook);
                        }
                    }
                }
            } else {
                println!("{}", serde_json::to_string_pretty(&integrations)?);
            }
        }
        Commands::Integration { slug } => {
            let client = get_client(site)?;
            let integration = client.get_integration(&slug).await?;
            println!("{}", serde_json::to_string_pretty(&integration)?);
        }
    }

    Ok(())
}
