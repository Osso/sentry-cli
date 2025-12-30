mod api;
mod config;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "sentry")]
#[command(about = "CLI tool to access Sentry API")]
struct Cli {
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
}

#[derive(Subcommand)]
enum IssueCommands {
    /// Get the latest event for this issue
    Latest,
    /// Get all events for this issue
    Events,
    /// Get hashes for this issue
    Hashes,
}

fn get_client() -> Result<api::Client> {
    let cfg = config::load_config()?;
    let org = cfg.organization.ok_or_else(|| {
        anyhow::anyhow!("Organization not configured. Run 'sentry config -o <org>' first")
    })?;
    let token = cfg.auth_token.ok_or_else(|| {
        anyhow::anyhow!(
            "Auth token not configured. Run 'sentry config -t <token>' or 'sentry-cli login' first"
        )
    })?;
    api::Client::new(&org, &token)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Config { org, token } => {
            let mut cfg = config::load_config().unwrap_or_default();

            if let Some(o) = org {
                cfg.organization = Some(o);
            }
            if let Some(t) = token {
                cfg.auth_token = Some(t);
            }

            config::save_config(&cfg)?;
            println!("Config saved to ~/.config/sentry-cli-rs/config.json");
        }
        Commands::Issue { id, command } => {
            let client = get_client()?;
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
                Some(IssueCommands::Hashes) => {
                    let hashes = client.get_issue_hashes(&id).await?;
                    println!("{}", serde_json::to_string_pretty(&hashes)?);
                }
            }
        }
        Commands::Projects => {
            let client = get_client()?;
            let projects = client.list_projects().await?;
            println!("{}", serde_json::to_string_pretty(&projects)?);
        }
        Commands::Issues { project, query } => {
            let client = get_client()?;
            let issues = client.list_issues(&project, query.as_deref()).await?;
            println!("{}", serde_json::to_string_pretty(&issues)?);
        }
    }

    Ok(())
}
