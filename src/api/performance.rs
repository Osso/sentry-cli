use super::Client;
use crate::performance::PerformanceMetric;
use anyhow::Result;
use serde_json::Value;

pub struct TransactionRankingRequest<'a> {
    pub project: &'a str,
    pub search_query: &'a str,
    pub period: &'a str,
    pub metric: PerformanceMetric,
    pub limit: u32,
}

fn transaction_ranking_endpoint(
    organization: &str,
    request: &TransactionRankingRequest<'_>,
) -> String {
    let metric_field = request.metric.aggregate_field();
    let mut endpoint = format!(
        "/organizations/{organization}/events/?dataset=spans&project={}&query={}&statsPeriod={}&per_page={}&sort={}",
        urlencoding::encode(request.project),
        urlencoding::encode(request.search_query),
        urlencoding::encode(request.period),
        request.limit,
        request.metric.sort_key()
    );
    for field in ["transaction", "count()", metric_field] {
        endpoint.push_str("&field=");
        endpoint.push_str(&urlencoding::encode(field));
    }
    endpoint
}

impl Client {
    /// Rank transaction durations through Sentry Explore.
    pub async fn search_transaction_rankings(
        &self,
        request: &TransactionRankingRequest<'_>,
    ) -> Result<Value> {
        self.get(&transaction_ranking_endpoint(&self.organization, request))
            .await
    }
}

#[cfg(test)]
pub(super) fn test_response(line: &str) -> (&'static str, String) {
    if line.contains("fail%3Atrue") {
        return (
            "400 Bad Request",
            "invalid transaction ranking query".to_string(),
        );
    }
    if line.contains("empty%3Atrue") {
        return (
            "200 OK",
            serde_json::json!({"data": [], "meta": {"dataset": "spans"}}).to_string(),
        );
    }
    (
        "200 OK",
        serde_json::json!({
            "data": [{
                "transaction": "GET /new",
                "count()": 42.0,
                "p95(span.duration)": 1234.5
            }],
            "meta": {"dataset": "spans"}
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::{MockSentry, handler};

    #[tokio::test]
    async fn client_queries_transaction_rankings_with_encoded_parameters_and_auth() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());
        let request = TransactionRankingRequest {
            project: "web app",
            search_query: "environment:production is_transaction:true transaction:\"GET /new\"",
            period: "24h",
            metric: PerformanceMetric::P95,
            limit: 20,
        };

        let ranking = client.search_transaction_rankings(&request).await.unwrap();

        assert_eq!(
            ranking,
            serde_json::json!({
                "data": [{
                    "transaction": "GET /new",
                    "count()": 42.0,
                    "p95(span.duration)": 1234.5
                }],
                "meta": {"dataset": "spans"}
            })
        );
        let sent_request = server
            .requests()
            .into_iter()
            .find(|request| {
                request.starts_with("get /api/0/organizations/test-org/events/?dataset=spans")
            })
            .expect("transaction ranking request should be sent");
        for parameter in [
            "dataset=spans",
            "project=web%20app",
            "query=environment%3aproduction%20is_transaction%3atrue%20transaction%3a%22get%20%2fnew%22",
            "statsperiod=24h",
            "per_page=20",
            "sort=-p95_span_duration",
            "field=transaction",
            "field=count%28%29",
            "field=p95%28span.duration%29",
        ] {
            assert!(
                sent_request.contains(parameter),
                "missing parameter: {parameter}"
            );
        }
        assert!(sent_request.contains("authorization: bearer test-token"));
    }

    #[tokio::test]
    async fn client_preserves_empty_transaction_ranking_result() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());
        let request = TransactionRankingRequest {
            project: "web",
            search_query: "environment:production is_transaction:true empty:true",
            period: "7d",
            metric: PerformanceMetric::Avg,
            limit: 25,
        };

        let ranking = client.search_transaction_rankings(&request).await.unwrap();

        assert_eq!(
            ranking,
            serde_json::json!({"data": [], "meta": {"dataset": "spans"}})
        );
    }

    #[tokio::test]
    async fn client_reports_transaction_ranking_api_failure_with_status_and_body() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());
        let request = TransactionRankingRequest {
            project: "web",
            search_query: "environment:production is_transaction:true fail:true",
            period: "24h",
            metric: PerformanceMetric::P99,
            limit: 10,
        };

        let error = client
            .search_transaction_rankings(&request)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("HTTP 400 Bad Request - invalid transaction ranking query")
        );
    }
}
