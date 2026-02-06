mod api;
mod config;

use anyhow::{bail, Result};
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
    /// Resolve an issue by ID (shortcut for `issue <id> resolve`)
    Resolve {
        /// Issue ID
        id: String,
    },
    /// Snooze an issue for a duration (shortcut for `issue <id> snooze`)
    Snooze {
        /// Issue ID
        id: String,
        /// Duration to snooze (e.g., 30d, 7d, 24h, 60m). Default: 30d
        #[arg(short, long, default_value = "30d")]
        duration: String,
    },
    /// List releases
    Releases {
        /// Filter by project slug
        #[arg(short, long)]
        project: Option<String>,
        /// Number of releases to fetch (default: 25)
        #[arg(short, long)]
        limit: Option<u32>,
    },
    /// Get release details
    Release {
        /// Release version (e.g., web36aa0c07)
        version: String,
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
    /// Unresolve issue (set back to unresolved from resolved or ignored)
    Unresolve,
    /// Ignore issue (archive - won't reopen on new events)
    Ignore,
    /// Unignore issue (set back to unresolved from ignored)
    Unignore,
    /// Snooze issue for a duration (ignored temporarily, reopens after)
    Snooze {
        /// Duration to snooze (e.g., 30d, 7d, 24h, 60m). Default: 30d
        #[arg(short, long, default_value = "30d")]
        duration: String,
    },
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

/// Parse a duration string like "30d", "7d", "24h", "60m" into minutes
fn parse_duration_to_minutes(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        bail!("Duration cannot be empty");
    }

    let (num_str, suffix) = s.split_at(s.len() - 1);
    let value: u64 = num_str
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid duration '{}'. Expected format: 30d, 24h, or 60m", s))?;

    match suffix {
        "d" => Ok(value * 24 * 60),
        "h" => Ok(value * 60),
        "m" => Ok(value),
        _ => bail!("Unknown duration suffix '{}'. Use d (days), h (hours), or m (minutes)", suffix),
    }
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
                        println!(
                            "Legacy config found (org: {})",
                            cfg.organization.as_deref().unwrap_or("?")
                        );
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
                Some(IssueCommands::Unresolve) => {
                    let result = client.unresolve_issue(&id).await?;
                    let short_id = result["shortId"].as_str().unwrap_or(&id);
                    println!("Unresolved {}", short_id);
                }
                Some(IssueCommands::Ignore) => {
                    let result = client.ignore_issue(&id).await?;
                    let short_id = result["shortId"].as_str().unwrap_or(&id);
                    println!("Ignored {}", short_id);
                }
                Some(IssueCommands::Unignore) => {
                    let result = client.unresolve_issue(&id).await?;
                    let short_id = result["shortId"].as_str().unwrap_or(&id);
                    println!("Unignored {}", short_id);
                }
                Some(IssueCommands::Snooze { duration }) => {
                    let minutes = parse_duration_to_minutes(&duration)?;
                    let result = client.snooze_issue(&id, minutes).await?;
                    let short_id = result["shortId"].as_str().unwrap_or(&id);
                    println!("Snoozed {} for {}", short_id, duration);
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
        Commands::Resolve { id } => {
            let client = get_client(site)?;
            let result = client.resolve_issue(&id).await?;
            let short_id = result["shortId"].as_str().unwrap_or(&id);
            println!("Resolved {}", short_id);
        }
        Commands::Snooze { id, duration } => {
            let client = get_client(site)?;
            let minutes = parse_duration_to_minutes(&duration)?;
            let result = client.snooze_issue(&id, minutes).await?;
            let short_id = result["shortId"].as_str().unwrap_or(&id);
            println!("Snoozed {} for {}", short_id, duration);
        }
        Commands::Releases { project, limit } => {
            let client = get_client(site)?;
            let releases = client.list_releases(project.as_deref(), limit).await?;
            println!("{}", serde_json::to_string_pretty(&releases)?);
        }
        Commands::Release { version } => {
            let client = get_client(site)?;
            let release = client.get_release(&version).await?;
            println!("{}", serde_json::to_string_pretty(&release)?);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_days() {
        assert_eq!(parse_duration_to_minutes("30d").unwrap(), 30 * 24 * 60);
        assert_eq!(parse_duration_to_minutes("7d").unwrap(), 7 * 24 * 60);
        assert_eq!(parse_duration_to_minutes("1d").unwrap(), 24 * 60);
    }

    #[test]
    fn test_parse_hours() {
        assert_eq!(parse_duration_to_minutes("24h").unwrap(), 24 * 60);
        assert_eq!(parse_duration_to_minutes("1h").unwrap(), 60);
    }

    #[test]
    fn test_parse_minutes() {
        assert_eq!(parse_duration_to_minutes("60m").unwrap(), 60);
        assert_eq!(parse_duration_to_minutes("30m").unwrap(), 30);
    }

    #[test]
    fn test_parse_invalid() {
        assert!(parse_duration_to_minutes("").is_err());
        assert!(parse_duration_to_minutes("abc").is_err());
        assert!(parse_duration_to_minutes("30x").is_err());
        assert!(parse_duration_to_minutes("d").is_err());
    }
}
