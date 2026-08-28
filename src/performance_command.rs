#[cfg(not(test))]
use crate::api;
use crate::performance;
use anyhow::{Result, bail};
use clap::Args;

#[derive(Args)]
pub struct PerformanceCommand {
    /// Project slug
    project: String,
    /// Sentry environment
    #[arg(short, long, default_value = "production")]
    environment: String,
    /// Relative time period (e.g., 24h, 7d, 30d)
    #[arg(short, long, default_value = "24h")]
    period: String,
    /// Additional Explore search query
    #[arg(short, long)]
    query: Option<String>,
    /// Duration aggregate used for ranking
    #[arg(short, long, value_enum, default_value = "p95")]
    metric: performance::PerformanceMetric,
    /// Maximum transactions to return
    #[arg(short, long, default_value_t = 20)]
    limit: u32,
    /// Print normalized JSON instead of a table
    #[arg(long)]
    json: bool,
}

fn validate_search(environment: &str, period: &str, limit: u32) -> Result<()> {
    if environment.trim().is_empty() {
        bail!("Environment cannot be empty");
    }
    if !(1..=100).contains(&limit) {
        bail!("Limit must be between 1 and 100");
    }
    if crate::parse_duration_to_minutes(period)? == 0 {
        bail!("Period must be greater than zero");
    }
    Ok(())
}

fn build_query(environment: &str, query: Option<&str>) -> String {
    let base_query = format!("environment:{} is_transaction:true", environment.trim());
    let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) else {
        return base_query;
    };
    format!("{base_query} {query}")
}

#[cfg(not(test))]
pub async fn query_and_print(client: &api::Client, command: PerformanceCommand) -> Result<()> {
    validate_search(&command.environment, &command.period, command.limit)?;
    let query = build_query(&command.environment, command.query.as_deref());
    let request = api::TransactionRankingRequest {
        project: &command.project,
        search_query: &query,
        period: &command.period,
        metric: command.metric,
        limit: command.limit,
    };
    let response = client.search_transaction_rankings(&request).await?;
    let ranking = performance::parse_performance_ranking(
        &response,
        &command.project,
        &command.environment,
        &command.period,
        &query,
        command.metric,
    )?;

    if command.json {
        println!("{}", serde_json::to_string_pretty(&ranking)?);
        return Ok(());
    }
    print!("{}", performance::format_performance_table(&ranking));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn performance_command_uses_operational_defaults() {
        let cli = Cli::try_parse_from(["sentry", "performance", "web"]).unwrap();

        let Commands::Performance(command) = cli.command else {
            panic!("performance command should parse");
        };

        assert_eq!(command.project, "web");
        assert_eq!(command.environment, "production");
        assert_eq!(command.period, "24h");
        assert_eq!(command.query, None);
        assert_eq!(command.metric, performance::PerformanceMetric::P95);
        assert_eq!(command.limit, 20);
        assert!(!command.json);
        validate_search(&command.environment, &command.period, command.limit).unwrap();
        assert_eq!(
            build_query(&command.environment, command.query.as_deref()),
            "environment:production is_transaction:true"
        );
    }

    #[test]
    fn performance_command_accepts_metric_scope_query_limit_and_json() {
        let cli = Cli::try_parse_from([
            "sentry",
            "performance",
            "web",
            "--environment",
            "staging",
            "--period",
            "7d",
            "--query",
            "transaction:www/*",
            "--metric",
            "p75",
            "--limit",
            "5",
            "--json",
        ])
        .unwrap();

        let Commands::Performance(command) = cli.command else {
            panic!("performance command should parse");
        };

        assert_eq!(command.project, "web");
        assert_eq!(command.environment, "staging");
        assert_eq!(command.period, "7d");
        assert_eq!(command.query.as_deref(), Some("transaction:www/*"));
        assert_eq!(command.metric, performance::PerformanceMetric::P75);
        assert_eq!(command.limit, 5);
        assert!(command.json);
        assert_eq!(
            build_query(&command.environment, command.query.as_deref()),
            "environment:staging is_transaction:true transaction:www/*"
        );
    }

    #[test]
    fn performance_command_rejects_invalid_scope_period_and_limit() {
        assert!(validate_search("production", "24h", 1).is_ok());
        assert!(validate_search("production", "7d", 100).is_ok());
        assert!(validate_search("", "24h", 20).is_err());
        assert!(validate_search("production", "0h", 20).is_err());
        assert!(validate_search("production", "invalid", 20).is_err());
        assert!(validate_search("production", "24h", 0).is_err());
        assert!(validate_search("production", "24h", 101).is_err());
    }

    #[test]
    fn performance_query_ignores_blank_optional_query() {
        assert_eq!(
            build_query("production", Some("   ")),
            "environment:production is_transaction:true"
        );
    }
}
