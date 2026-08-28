mod api;
mod config;
mod performance;
mod trace;

#[cfg(not(test))]
use std::collections::HashMap;

use anyhow::{Result, bail};
use chrono::{DateTime, FixedOffset};
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
    /// Search error events across a project
    Events {
        /// Project slug
        project: String,
        /// Explore search query
        #[arg(short, long)]
        query: Option<String>,
        /// Range start as an RFC3339 UTC timestamp
        #[arg(long)]
        start: String,
        /// Range end as an RFC3339 UTC timestamp
        #[arg(long)]
        end: String,
        /// Maximum events to return
        #[arg(short, long, default_value_t = 100)]
        limit: u32,
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
    /// Get trace details
    Trace {
        /// Trace ID
        trace_id: String,
    },
    /// Get event details and display spans
    Event {
        /// Project slug (e.g., web)
        project: String,
        /// Event ID
        event_id: String,
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
    /// Delete this monitor
    Delete,
}

/// Parse a duration string like "30d", "7d", "24h", "60m" into minutes
fn parse_duration_to_minutes(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        bail!("Duration cannot be empty");
    }

    let (num_str, suffix) = s.split_at(s.len() - 1);
    let value: u64 = num_str.parse().map_err(|_| {
        anyhow::anyhow!(
            "Invalid duration '{}'. Expected format: 30d, 24h, or 60m",
            s
        )
    })?;

    match suffix {
        "d" => Ok(value * 24 * 60),
        "h" => Ok(value * 60),
        "m" => Ok(value),
        _ => bail!(
            "Unknown duration suffix '{}'. Use d (days), h (hours), or m (minutes)",
            suffix
        ),
    }
}

fn validate_event_search(start: &str, end: &str, limit: u32) -> Result<()> {
    if !(1..=100).contains(&limit) {
        bail!("Limit must be between 1 and 100");
    }

    let start = parse_utc_timestamp("start", start)?;
    let end = parse_utc_timestamp("end", end)?;
    if start >= end {
        bail!("Start timestamp must be before end timestamp");
    }
    Ok(())
}

fn parse_utc_timestamp(label: &str, value: &str) -> Result<DateTime<FixedOffset>> {
    if !value.ends_with('Z') && !value.ends_with("+00:00") {
        bail!("{label} must use the UTC offset Z or +00:00");
    }
    DateTime::parse_from_rfc3339(value)
        .map_err(|error| anyhow::anyhow!("Invalid {label} RFC3339 timestamp: {error}"))
}

#[cfg(not(test))]
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

#[cfg(not(test))]
fn print_config_sites(cfg: &config::Config) {
    let sites = cfg.list_sites();
    if sites.is_empty() {
        println!("No sites configured.");
        if cfg.organization.is_some() {
            println!(
                "Legacy config found (org: {})",
                cfg.organization.as_deref().unwrap_or("?")
            );
        }
        return;
    }
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

#[cfg(not(test))]
fn handle_config_save(
    cfg: &mut config::Config,
    site: Option<&str>,
    org: Option<String>,
    token: Option<String>,
    default: bool,
) -> Result<()> {
    if let Some(site_name) = site {
        return save_site_config(cfg, site_name, org, token, default);
    }

    if org.is_some() || token.is_some() {
        return save_legacy_config(cfg, org, token);
    }

    print_config_summary(cfg);
    Ok(())
}

#[cfg(not(test))]
fn save_site_config(
    cfg: &mut config::Config,
    site: &str,
    org: Option<String>,
    token: Option<String>,
    default: bool,
) -> Result<()> {
    cfg.set_site(site, org, token);
    if default {
        cfg.default_site = Some(site.to_string());
    }
    config::save_config(cfg)?;
    println!("Config saved for site '{}'", site);
    Ok(())
}

#[cfg(not(test))]
fn save_legacy_config(
    cfg: &mut config::Config,
    org: Option<String>,
    token: Option<String>,
) -> Result<()> {
    if let Some(o) = org {
        cfg.organization = Some(o);
    }
    if let Some(t) = token {
        cfg.auth_token = Some(t);
    }
    config::save_config(cfg)?;
    println!("Config saved to ~/.config/sentry/config.json");
    Ok(())
}

#[cfg(not(test))]
fn print_config_summary(cfg: &config::Config) {
    if let Some(site_name) = &cfg.default_site {
        println!("Default site: {}", site_name);
    }

    let sites = cfg.list_sites();
    if !sites.is_empty() {
        println!("Sites: {}", sites.join(", "));
    }

    if let Some(org) = cfg.organization.as_deref() {
        println!("Legacy org: {}", org);
    }
}

#[cfg(not(test))]
fn print_issue_action(action: &str, issue: &serde_json::Value, fallback: &str) {
    let short_id = issue["shortId"].as_str().unwrap_or(fallback);
    println!("{} {}", action, short_id);
}

#[cfg(not(test))]
async fn handle_issue_status_change(
    client: &api::Client,
    id: &str,
    command: IssueCommands,
) -> Result<()> {
    match command {
        IssueCommands::Resolve => {
            let issue = client.resolve_issue(id).await?;
            print_issue_action("Resolved", &issue, id);
        }
        IssueCommands::Unresolve => {
            let issue = client.unresolve_issue(id).await?;
            print_issue_action("Unresolved", &issue, id);
        }
        IssueCommands::Ignore => {
            let issue = client.ignore_issue(id).await?;
            print_issue_action("Ignored", &issue, id);
        }
        IssueCommands::Unignore => {
            let issue = client.unresolve_issue(id).await?;
            print_issue_action("Unignored", &issue, id);
        }
        IssueCommands::Snooze { duration } => {
            let minutes = parse_duration_to_minutes(&duration)?;
            let result = client.snooze_issue(id, minutes).await?;
            let short_id = result["shortId"].as_str().unwrap_or(id);
            println!("Snoozed {} for {}", short_id, duration);
        }
        _ => unreachable!(),
    }
    Ok(())
}

#[cfg(not(test))]
async fn handle_issue_command(
    client: &api::Client,
    id: &str,
    command: Option<IssueCommands>,
) -> Result<()> {
    match command {
        None => print_json(&client.get_issue(id).await?)?,
        Some(IssueCommands::Latest) => print_json(&client.get_issue_latest_event(id).await?)?,
        Some(IssueCommands::Events) => print_json(&client.get_issue_events(id).await?)?,
        Some(IssueCommands::Event { event_id }) => {
            print_json(&client.get_issue_event(id, &event_id).await?)?
        }
        Some(IssueCommands::Hashes) => print_json(&client.get_issue_hashes(id).await?)?,
        Some(cmd) => handle_issue_status_change(client, id, cmd).await?,
    }
    Ok(())
}

#[cfg(not(test))]
fn print_integrations(integrations: &serde_json::Value) -> Result<()> {
    let Some(items) = integrations.as_array() else {
        println!("{}", serde_json::to_string_pretty(integrations)?);
        return Ok(());
    };

    if items.is_empty() {
        println!("No internal integrations found.");
        return Ok(());
    }

    for integration in items {
        let name = integration["name"].as_str().unwrap_or("?");
        let slug = integration["slug"].as_str().unwrap_or("?");
        let webhook = integration["webhookUrl"].as_str().unwrap_or("-");
        println!("{} ({})", name, slug);
        if webhook != "-" {
            println!("  webhook: {}", webhook);
        }
    }

    Ok(())
}

#[cfg(not(test))]
async fn fetch_trace_events(
    client: &api::Client,
    data: &serde_json::Value,
) -> HashMap<String, serde_json::Value> {
    let mut events = HashMap::new();
    for (span_id, project_slug, event_id) in trace::extract_transaction_event_refs(data) {
        match client.get_event(&project_slug, &event_id).await {
            Ok(event) => {
                events.insert(span_id, event);
            }
            Err(e) => eprintln!("Warning: failed to fetch event {}: {}", event_id, e),
        }
    }
    events
}

#[cfg(not(test))]
async fn handle_trace_command(client: &api::Client, trace_id: &str) -> Result<()> {
    let data = client.get_trace(trace_id).await?;
    let events = fetch_trace_events(client, &data).await;
    trace::print_trace(&data, &events);
    Ok(())
}

fn print_json(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[cfg(not(test))]
async fn handle_resolve_shortcut(client: &api::Client, id: &str) -> Result<()> {
    println!(
        "Resolved {}",
        client.resolve_issue(id).await?["shortId"]
            .as_str()
            .unwrap_or(id)
    );
    Ok(())
}

#[cfg(not(test))]
async fn handle_snooze_shortcut(client: &api::Client, id: &str, duration: &str) -> Result<()> {
    let minutes = parse_duration_to_minutes(duration)?;
    let result = client.snooze_issue(id, minutes).await?;
    println!(
        "Snoozed {} for {}",
        result["shortId"].as_str().unwrap_or(id),
        duration
    );
    Ok(())
}

#[cfg(not(test))]
async fn handle_event_command(client: &api::Client, project: &str, event_id: &str) -> Result<()> {
    let event = client.get_event(project, event_id).await?;
    trace::print_event_spans(&event);
    Ok(())
}

#[cfg(not(test))]
async fn search_project_events(
    client: &api::Client,
    project: &str,
    query: Option<&str>,
    start: &str,
    end: &str,
    limit: u32,
) -> Result<()> {
    validate_event_search(start, end, limit)?;
    print_json(
        &client
            .search_events(project, query, start, end, limit)
            .await?,
    )
}

#[cfg(not(test))]
fn handle_config_command(
    site: Option<&str>,
    org: Option<String>,
    token: Option<String>,
    default: bool,
    list: bool,
) -> Result<()> {
    let mut cfg = config::load_config().unwrap_or_default();
    if list {
        print_config_sites(&cfg);
        return Ok(());
    }
    handle_config_save(&mut cfg, site, org, token, default)
}

#[cfg(not(test))]
async fn dispatch_client_core(client: &api::Client, command: Commands) -> Result<()> {
    match command {
        Commands::Issue { id, command } => handle_issue_command(client, &id, command).await?,
        Commands::Projects => print_json(&client.list_projects().await?)?,
        Commands::Issues { project, query } => {
            print_json(&client.list_issues(&project, query.as_deref()).await?)?
        }
        Commands::Events {
            project,
            query,
            start,
            end,
            limit,
        } => search_project_events(client, &project, query.as_deref(), &start, &end, limit).await?,
        Commands::Monitors { environment } => {
            print_json(&client.list_monitors(environment.as_deref()).await?)?
        }
        Commands::Monitor { slug, command } => {
            handle_monitor_command(client, &slug, command).await?
        }
        other => dispatch_client_extended(client, other).await?,
    }
    Ok(())
}

#[cfg(not(test))]
async fn dispatch_client_extended(client: &api::Client, command: Commands) -> Result<()> {
    match command {
        Commands::Integrations
        | Commands::Integration { .. }
        | Commands::Resolve { .. }
        | Commands::Snooze { .. } => dispatch_client_management(client, command).await?,
        Commands::Releases { .. }
        | Commands::Release { .. }
        | Commands::Trace { .. }
        | Commands::Event { .. } => dispatch_client_inspection(client, command).await?,
        Commands::Issue { .. }
        | Commands::Projects
        | Commands::Issues { .. }
        | Commands::Monitors { .. }
        | Commands::Monitor { .. }
        | Commands::Config { .. }
        | Commands::Events { .. } => unreachable!(),
    }
    Ok(())
}

#[cfg(not(test))]
async fn dispatch_client_management(client: &api::Client, command: Commands) -> Result<()> {
    match command {
        Commands::Integrations => print_integrations(&client.list_integrations().await?)?,
        Commands::Integration { slug } => print_json(&client.get_integration(&slug).await?)?,
        Commands::Resolve { id } => handle_resolve_shortcut(client, &id).await?,
        Commands::Snooze { id, duration } => handle_snooze_shortcut(client, &id, &duration).await?,
        _ => unreachable!(),
    }
    Ok(())
}

#[cfg(not(test))]
async fn dispatch_client_inspection(client: &api::Client, command: Commands) -> Result<()> {
    match command {
        Commands::Releases { project, limit } => {
            print_json(&client.list_releases(project.as_deref(), limit).await?)?
        }
        Commands::Release { version } => print_json(&client.get_release(&version).await?)?,
        Commands::Trace { trace_id } => handle_trace_command(client, &trace_id).await?,
        Commands::Event { project, event_id } => {
            handle_event_command(client, &project, &event_id).await?
        }
        _ => unreachable!(),
    }
    Ok(())
}

#[cfg(not(test))]
async fn dispatch(cli: Cli) -> Result<()> {
    let site = cli.site.as_deref();
    match cli.command {
        Commands::Config {
            org,
            token,
            default,
            list,
        } => handle_config_command(site, org, token, default, list)?,
        command => {
            let client = get_client(site)?;
            dispatch_client_core(&client, command).await?;
        }
    }
    Ok(())
}

#[cfg(not(test))]
async fn handle_monitor_command(
    client: &api::Client,
    slug: &str,
    command: Option<MonitorCommands>,
) -> Result<()> {
    match command {
        None => println!(
            "{}",
            serde_json::to_string_pretty(&client.get_monitor(slug).await?)?
        ),
        Some(MonitorCommands::Checkins { limit }) => println!(
            "{}",
            serde_json::to_string_pretty(&client.list_monitor_checkins(slug, limit).await?)?
        ),
        Some(MonitorCommands::Delete) => {
            client.delete_monitor(slug).await?;
            println!("Monitor '{}' deleted successfully.", slug);
        }
    }
    Ok(())
}

#[cfg(not(test))]
#[tokio::main]
async fn main() -> Result<()> {
    dispatch(Cli::parse()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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

    #[test]
    fn events_command_accepts_valid_utc_range_and_defaults_limit() {
        let cli = Cli::try_parse_from([
            "sentry",
            "events",
            "flutter",
            "--query",
            "user.id:762159",
            "--start",
            "2026-08-12T16:27:00Z",
            "--end",
            "2026-08-12T16:29:00+00:00",
        ])
        .unwrap();

        let Commands::Events {
            project,
            query,
            start,
            end,
            limit,
        } = cli.command
        else {
            panic!("events command should parse");
        };

        assert_eq!(project, "flutter");
        assert_eq!(query.as_deref(), Some("user.id:762159"));
        assert_eq!(limit, 100);
        validate_event_search(&start, &end, limit).unwrap();
    }

    #[test]
    fn events_command_rejects_malformed_and_non_utc_ranges() {
        assert!(validate_event_search("not-a-timestamp", "2026-08-12T16:29:00Z", 100,).is_err());
        assert!(
            validate_event_search("2026-08-12T16:27:00-04:00", "2026-08-12T16:29:00Z", 100,)
                .is_err()
        );
    }

    #[test]
    fn events_command_rejects_start_at_or_after_end() {
        assert!(
            validate_event_search("2026-08-12T16:29:00Z", "2026-08-12T16:29:00Z", 100,).is_err()
        );
        assert!(
            validate_event_search("2026-08-12T16:30:00Z", "2026-08-12T16:29:00Z", 100,).is_err()
        );
    }

    #[test]
    fn events_command_enforces_limit_boundaries() {
        assert!(validate_event_search("2026-08-12T16:27:00Z", "2026-08-12T16:29:00Z", 1,).is_ok());
        assert!(
            validate_event_search("2026-08-12T16:27:00Z", "2026-08-12T16:29:00Z", 100,).is_ok()
        );
        assert!(validate_event_search("2026-08-12T16:27:00Z", "2026-08-12T16:29:00Z", 0,).is_err());
        assert!(
            validate_event_search("2026-08-12T16:27:00Z", "2026-08-12T16:29:00Z", 101,).is_err()
        );
    }

    #[test]
    fn test_print_json_accepts_values() {
        print_json(&serde_json::json!({"ok": true})).unwrap();
    }
}
