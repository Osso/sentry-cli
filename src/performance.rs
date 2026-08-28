use anyhow::{Result, bail};
use clap::ValueEnum;
use serde::Serialize;
use serde_json::Value;
use std::fmt::{self, Display};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum PerformanceMetric {
    Avg,
    P75,
    P95,
    P99,
}

impl Display for PerformanceMetric {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl PerformanceMetric {
    pub fn name(self) -> &'static str {
        match self {
            Self::Avg => "avg",
            Self::P75 => "p75",
            Self::P95 => "p95",
            Self::P99 => "p99",
        }
    }

    pub fn aggregate_field(self) -> &'static str {
        match self {
            Self::Avg => "avg(span.duration)",
            Self::P75 => "p75(span.duration)",
            Self::P95 => "p95(span.duration)",
            Self::P99 => "p99(span.duration)",
        }
    }

    pub fn sort_key(self) -> &'static str {
        match self {
            Self::Avg => "-avg_span_duration",
            Self::P75 => "-p75_span_duration",
            Self::P95 => "-p95_span_duration",
            Self::P99 => "-p99_span_duration",
        }
    }

    pub fn table_label(self) -> &'static str {
        match self {
            Self::Avg => "AVG",
            Self::P75 => "P75",
            Self::P95 => "P95",
            Self::P99 => "P99",
        }
    }
}

#[derive(Debug, PartialEq, Serialize)]
pub struct PerformanceRow {
    pub transaction: String,
    pub count: u64,
    pub duration_ms: f64,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct PerformanceRanking {
    pub project: String,
    pub environment: String,
    pub period: String,
    pub query: String,
    pub metric: PerformanceMetric,
    pub rows: Vec<PerformanceRow>,
}

pub fn parse_performance_ranking(
    response: &Value,
    project: &str,
    environment: &str,
    period: &str,
    query: &str,
    metric: PerformanceMetric,
) -> Result<PerformanceRanking> {
    let data = response
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("Sentry performance response is missing data"))?
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Sentry performance response data must be an array"))?;

    let mut rows = data
        .iter()
        .enumerate()
        .map(|(index, row)| parse_performance_row(row, index + 1, metric))
        .collect::<Result<Vec<_>>>()?;
    rows.sort_by(|left, right| {
        right
            .duration_ms
            .total_cmp(&left.duration_ms)
            .then_with(|| left.transaction.cmp(&right.transaction))
    });

    Ok(PerformanceRanking {
        project: project.to_string(),
        environment: environment.to_string(),
        period: period.to_string(),
        query: query.to_string(),
        metric,
        rows,
    })
}

fn parse_performance_row(
    row: &Value,
    row_number: usize,
    metric: PerformanceMetric,
) -> Result<PerformanceRow> {
    let transaction = row
        .get("transaction")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Sentry performance row {row_number} field transaction must be a non-empty string"
            )
        })?;
    let count = parse_count(row.get("count()"), row_number)?;
    let duration_ms = parse_duration(row.get(metric.aggregate_field()), row_number, metric)?;

    Ok(PerformanceRow {
        transaction: transaction.to_string(),
        count,
        duration_ms,
    })
}

fn parse_count(value: Option<&Value>, row_number: usize) -> Result<u64> {
    let Some(value) = value else {
        bail!("Sentry performance row {row_number} is missing field count()");
    };
    if let Some(count) = value.as_u64() {
        return Ok(count);
    }

    let Some(count) = value.as_f64() else {
        bail!(
            "Sentry performance row {row_number} field count() must be a whole nonnegative number"
        );
    };
    let is_valid =
        count.is_finite() && count >= 0.0 && count.fract() == 0.0 && count <= u64::MAX as f64;
    if !is_valid {
        bail!(
            "Sentry performance row {row_number} field count() must be a whole nonnegative number"
        );
    }

    Ok(count as u64)
}

fn parse_duration(
    value: Option<&Value>,
    row_number: usize,
    metric: PerformanceMetric,
) -> Result<f64> {
    let field = metric.aggregate_field();
    let duration = value.and_then(Value::as_f64).ok_or_else(|| {
        anyhow::anyhow!(
            "Sentry performance row {row_number} field {field} must be a finite nonnegative number"
        )
    })?;
    if !duration.is_finite() || duration < 0.0 {
        bail!(
            "Sentry performance row {row_number} field {field} must be a finite nonnegative number"
        );
    }

    Ok(duration)
}

pub fn format_performance_table(ranking: &PerformanceRanking) -> String {
    if ranking.rows.is_empty() {
        return "No performance transactions found.\n".to_string();
    }

    let mut table = format!(
        "RANK\tTRANSACTION\tCOUNT\t{} (MS)\n",
        ranking.metric.table_label()
    );
    for (index, row) in ranking.rows.iter().enumerate() {
        table.push_str(&format!(
            "{}\t{}\t{}\t{:.2}\n",
            index + 1,
            row.transaction,
            row.count,
            row.duration_ms
        ));
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;
    use serde_json::json;

    #[test]
    fn metrics_map_to_cli_api_json_and_table_contracts() {
        let cases = [
            (
                PerformanceMetric::Avg,
                "avg",
                "avg(span.duration)",
                "-avg_span_duration",
                "AVG",
            ),
            (
                PerformanceMetric::P75,
                "p75",
                "p75(span.duration)",
                "-p75_span_duration",
                "P75",
            ),
            (
                PerformanceMetric::P95,
                "p95",
                "p95(span.duration)",
                "-p95_span_duration",
                "P95",
            ),
            (
                PerformanceMetric::P99,
                "p99",
                "p99(span.duration)",
                "-p99_span_duration",
                "P99",
            ),
        ];

        for (metric, name, aggregate_field, sort_key, table_label) in cases {
            assert_eq!(PerformanceMetric::from_str(name, true), Ok(metric));
            assert_eq!(metric.to_string(), name);
            assert_eq!(metric.aggregate_field(), aggregate_field);
            assert_eq!(metric.sort_key(), sort_key);
            assert_eq!(metric.table_label(), table_label);
            assert_eq!(serde_json::to_value(metric).unwrap(), json!(name));
        }
    }

    #[test]
    fn parses_and_sorts_rankings_by_duration_then_transaction() {
        let response = json!({
            "data": [
                {"transaction": "route-z", "count()": 4, "p95(span.duration)": 120.5},
                {"transaction": "route-b", "count()": 2.0, "p95(span.duration)": 300.0},
                {"transaction": "route-a", "count()": 3, "p95(span.duration)": 300.0}
            ]
        });

        let ranking = parse_performance_ranking(
            &response,
            "web",
            "production",
            "24h",
            "environment:production is_transaction:true",
            PerformanceMetric::P95,
        )
        .unwrap();

        assert_eq!(ranking.project, "web");
        assert_eq!(ranking.environment, "production");
        assert_eq!(ranking.period, "24h");
        assert_eq!(ranking.query, "environment:production is_transaction:true");
        assert_eq!(ranking.metric, PerformanceMetric::P95);
        assert_eq!(
            ranking.rows,
            vec![
                PerformanceRow {
                    transaction: "route-a".to_string(),
                    count: 3,
                    duration_ms: 300.0,
                },
                PerformanceRow {
                    transaction: "route-b".to_string(),
                    count: 2,
                    duration_ms: 300.0,
                },
                PerformanceRow {
                    transaction: "route-z".to_string(),
                    count: 4,
                    duration_ms: 120.5,
                },
            ]
        );
    }

    #[test]
    fn accepts_an_empty_data_array() {
        let ranking = parse_performance_ranking(
            &json!({"data": []}),
            "web",
            "production",
            "24h",
            "environment:production is_transaction:true",
            PerformanceMetric::P95,
        )
        .unwrap();

        assert!(ranking.rows.is_empty());
        assert_eq!(
            format_performance_table(&ranking),
            "No performance transactions found.\n"
        );
    }

    #[test]
    fn rejects_missing_or_malformed_response_data() {
        let missing = parse_performance_ranking(
            &json!({}),
            "web",
            "production",
            "24h",
            "query",
            PerformanceMetric::P95,
        )
        .unwrap_err();
        assert!(missing.to_string().contains("data"));

        let wrong_type = parse_performance_ranking(
            &json!({"data": {}}),
            "web",
            "production",
            "24h",
            "query",
            PerformanceMetric::P95,
        )
        .unwrap_err();
        assert!(wrong_type.to_string().contains("array"));
    }

    #[test]
    fn rejects_missing_or_empty_transactions_with_row_context() {
        for response in [
            json!({"data": [{"count()": 1, "p95(span.duration)": 10.0}]}),
            json!({"data": [{"transaction": "  ", "count()": 1, "p95(span.duration)": 10.0}]}),
        ] {
            let error = parse_performance_ranking(
                &response,
                "web",
                "production",
                "24h",
                "query",
                PerformanceMetric::P95,
            )
            .unwrap_err();

            assert!(error.to_string().contains("row 1"));
            assert!(error.to_string().contains("transaction"));
        }
    }

    #[test]
    fn rejects_invalid_counts_with_row_and_field_context() {
        for count in [json!(-1), json!(1.5), json!("3")] {
            let response = json!({
                "data": [{
                    "transaction": "route",
                    "count()": count,
                    "p95(span.duration)": 10.0
                }]
            });
            let error = parse_performance_ranking(
                &response,
                "web",
                "production",
                "24h",
                "query",
                PerformanceMetric::P95,
            )
            .unwrap_err();

            assert!(error.to_string().contains("row 1"));
            assert!(error.to_string().contains("count()"));
        }
    }

    #[test]
    fn rejects_invalid_durations_with_row_and_field_context() {
        for duration in [json!(-0.1), json!("NaN"), json!(null)] {
            let response = json!({
                "data": [{
                    "transaction": "route",
                    "count()": 1,
                    "p95(span.duration)": duration
                }]
            });
            let error = parse_performance_ranking(
                &response,
                "web",
                "production",
                "24h",
                "query",
                PerformanceMetric::P95,
            )
            .unwrap_err();

            assert!(error.to_string().contains("row 1"));
            assert!(error.to_string().contains("p95(span.duration)"));
        }
    }

    #[test]
    fn serializes_a_stable_normalized_ranking() {
        let ranking = PerformanceRanking {
            project: "web".to_string(),
            environment: "production".to_string(),
            period: "24h".to_string(),
            query: "environment:production is_transaction:true".to_string(),
            metric: PerformanceMetric::P95,
            rows: vec![PerformanceRow {
                transaction: "api/v1/comics".to_string(),
                count: 8,
                duration_ms: 1250.25,
            }],
        };

        assert_eq!(
            serde_json::to_value(ranking).unwrap(),
            json!({
                "project": "web",
                "environment": "production",
                "period": "24h",
                "query": "environment:production is_transaction:true",
                "metric": "p95",
                "rows": [{
                    "transaction": "api/v1/comics",
                    "count": 8,
                    "duration_ms": 1250.25
                }]
            })
        );
    }

    #[test]
    fn formats_full_transaction_names_in_a_ranked_table() {
        let ranking = PerformanceRanking {
            project: "web".to_string(),
            environment: "production".to_string(),
            period: "24h".to_string(),
            query: "environment:production is_transaction:true".to_string(),
            metric: PerformanceMetric::P95,
            rows: vec![
                PerformanceRow {
                    transaction: "a/very/long/transaction/name/that/must/not/be/truncated"
                        .to_string(),
                    count: 12,
                    duration_ms: 4321.25,
                },
                PerformanceRow {
                    transaction: "short".to_string(),
                    count: 2,
                    duration_ms: 40.0,
                },
            ],
        };

        let table = format_performance_table(&ranking);

        assert!(table.contains("RANK"));
        assert!(table.contains("TRANSACTION"));
        assert!(table.contains("COUNT"));
        assert!(table.contains("P95 (MS)"));
        assert!(table.contains("1"));
        assert!(table.contains("a/very/long/transaction/name/that/must/not/be/truncated"));
        assert!(table.contains("12"));
        assert!(table.contains("4321.25"));
        assert!(table.contains("2"));
        assert!(table.contains("short"));
        assert!(table.contains("40.00"));
    }
}
