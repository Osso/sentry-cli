mod performance;

#[cfg(not(test))]
pub use performance::TransactionRankingRequest;

use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;

const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_SECS: u64 = 1;
const EVENT_SEARCH_FIELDS: [&str; 6] = ["id", "timestamp", "title", "issue", "user.id", "message"];
const SPAN_SEARCH_FIELDS: [&str; 8] = [
    "timestamp",
    "transaction",
    "trace",
    "id",
    "span_id",
    "span.op",
    "span.description",
    "span.duration",
];

pub struct SpanSearchRequest<'a> {
    pub project: &'a str,
    pub query: Option<&'a str>,
    pub start: &'a str,
    pub end: &'a str,
    pub limit: u32,
    pub fields: Option<&'a [String]>,
    pub sort: &'a str,
}

fn event_search_endpoint(
    organization: &str,
    project: &str,
    query: Option<&str>,
    start: &str,
    end: &str,
    limit: u32,
) -> String {
    let mut endpoint = format!(
        "/organizations/{organization}/events/?dataset=errors&project={}",
        urlencoding::encode(project)
    );
    if let Some(query) = query {
        endpoint.push_str("&query=");
        endpoint.push_str(&urlencoding::encode(query));
    }
    endpoint.push_str(&format!(
        "&start={}&end={}&per_page={limit}&sort=-timestamp",
        urlencoding::encode(start),
        urlencoding::encode(end)
    ));
    for field in EVENT_SEARCH_FIELDS {
        endpoint.push_str("&field=");
        endpoint.push_str(field);
    }
    endpoint
}

fn span_search_endpoint(organization: &str, request: &SpanSearchRequest<'_>) -> String {
    let mut endpoint = format!(
        "/organizations/{organization}/events/?dataset=spans&project={}",
        urlencoding::encode(request.project)
    );
    if let Some(query) = request.query {
        endpoint.push_str("&query=");
        endpoint.push_str(&urlencoding::encode(query));
    }
    endpoint.push_str(&format!(
        "&start={}&end={}&per_page={}&sort={}",
        urlencoding::encode(request.start),
        urlencoding::encode(request.end),
        request.limit,
        urlencoding::encode(request.sort),
    ));
    append_span_search_fields(&mut endpoint, request.fields);
    endpoint
}

fn append_span_search_fields(endpoint: &mut String, fields: Option<&[String]>) {
    if let Some(fields) = fields.filter(|fields| !fields.is_empty()) {
        for field in fields {
            endpoint.push_str("&field=");
            endpoint.push_str(&urlencoding::encode(field));
        }
        return;
    }
    for field in SPAN_SEARCH_FIELDS {
        endpoint.push_str("&field=");
        endpoint.push_str(field);
    }
}

pub struct Client {
    http: reqwest::Client,
    base_url: String,
    auth_token: String,
    organization: String,
}

/// Check if an error is a connection timeout (os error 110)
fn is_connection_timeout(err: &reqwest::Error) -> bool {
    // Check the error chain for connection timeout indicators
    let err_string = format!("{:?}", err);
    err_string.contains("os error 110") || err_string.contains("Connection timed out")
}

impl Client {
    #[cfg(not(test))]
    pub fn new(organization: &str, auth_token: &str) -> Result<Self> {
        let http = reqwest::Client::builder().build()?;

        Ok(Self {
            http,
            base_url: "https://sentry.io/api/0".to_string(),
            auth_token: auth_token.to_string(),
            organization: organization.to_string(),
        })
    }

    #[cfg(test)]
    fn for_test(base_url: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("test client should build"),
            base_url,
            auth_token: "test-token".to_string(),
            organization: "test-org".to_string(),
        }
    }

    fn authorized_request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, url)
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .header("Content-Type", "application/json")
    }

    async fn ensure_success(&self, resp: reqwest::Response) -> Result<reqwest::Response> {
        if resp.status().is_success() {
            return Ok(resp);
        }

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("HTTP {} - {}", status, body);
    }

    async fn send(&self, method: reqwest::Method, endpoint: &str) -> Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url, endpoint);
        let resp = self
            .send_with_retry(|| self.authorized_request(method.clone(), &url).send())
            .await
            .context("Failed to send request")?;

        self.ensure_success(resp).await
    }

    async fn get(&self, endpoint: &str) -> Result<Value> {
        self.send(reqwest::Method::GET, endpoint)
            .await?
            .json()
            .await
            .context("Failed to parse JSON response")
    }

    async fn delete(&self, endpoint: &str) -> Result<reqwest::StatusCode> {
        Ok(self.send(reqwest::Method::DELETE, endpoint).await?.status())
    }

    async fn put(&self, endpoint: &str, body: &Value) -> Result<Value> {
        let url = format!("{}{}", self.base_url, endpoint);
        let resp = self
            .send_with_retry(|| {
                self.authorized_request(reqwest::Method::PUT, &url)
                    .json(body)
                    .send()
            })
            .await
            .context("Failed to send request")?;
        let resp = self.ensure_success(resp).await?;
        resp.json().await.context("Failed to parse JSON response")
    }

    /// Send an HTTP request with retry logic for connection timeouts.
    /// Retries up to MAX_RETRIES times with exponential backoff.
    async fn send_with_retry<F, Fut>(
        &self,
        make_request: F,
    ) -> Result<reqwest::Response, reqwest::Error>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
    {
        for attempt in 0..=MAX_RETRIES {
            match make_request().await {
                Ok(resp) => return Ok(resp),
                Err(err) if attempt < MAX_RETRIES && is_connection_timeout(&err) => {
                    let delay = INITIAL_BACKOFF_SECS * 2u64.pow(attempt);
                    eprintln!(
                        "Connection timeout, retrying ({}/{})...",
                        attempt + 1,
                        MAX_RETRIES
                    );
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                }
                Err(err) => return Err(err),
            }
        }

        unreachable!("retry loop should return success or error")
    }

    /// Get issue details by ID
    pub async fn get_issue(&self, issue_id: &str) -> Result<Value> {
        self.get(&format!(
            "/organizations/{}/issues/{}/",
            self.organization, issue_id
        ))
        .await
    }

    /// Get latest event for an issue
    pub async fn get_issue_latest_event(&self, issue_id: &str) -> Result<Value> {
        self.get(&format!("/issues/{}/events/latest/", issue_id))
            .await
    }

    /// Get events for an issue
    pub async fn get_issue_events(&self, issue_id: &str) -> Result<Value> {
        self.get(&format!("/issues/{}/events/", issue_id)).await
    }

    /// Get a specific event by ID
    pub async fn get_issue_event(&self, issue_id: &str, event_id: &str) -> Result<Value> {
        self.get(&format!("/issues/{}/events/{}/", issue_id, event_id))
            .await
    }

    /// Get hashes for an issue
    pub async fn get_issue_hashes(&self, issue_id: &str) -> Result<Value> {
        self.get(&format!("/issues/{}/hashes/", issue_id)).await
    }

    /// List projects in the organization
    pub async fn list_projects(&self) -> Result<Value> {
        self.get(&format!("/organizations/{}/projects/", self.organization))
            .await
    }

    /// List issues for a project
    pub async fn list_issues(&self, project_slug: &str, query: Option<&str>) -> Result<Value> {
        let query_param = query.unwrap_or("is:unresolved");
        self.get(&format!(
            "/projects/{}/{}/issues/?query={}",
            self.organization,
            project_slug,
            urlencoding::encode(query_param)
        ))
        .await
    }

    /// Search project error events through Sentry Explore.
    pub async fn search_events(
        &self,
        project_slug: &str,
        query: Option<&str>,
        start: &str,
        end: &str,
        limit: u32,
    ) -> Result<Value> {
        self.get(&event_search_endpoint(
            &self.organization,
            project_slug,
            query,
            start,
            end,
            limit,
        ))
        .await
    }

    /// Search individual transaction and span rows through Sentry Explore.
    pub async fn search_spans(&self, request: &SpanSearchRequest<'_>) -> Result<Value> {
        self.get(&span_search_endpoint(&self.organization, request))
            .await
    }

    /// Resolve an issue (accepts short ID like "WEB-81D" or numeric ID)
    pub async fn resolve_issue(&self, issue_id: &str) -> Result<Value> {
        self.update_issue_status(issue_id, "resolved").await
    }

    /// Ignore an issue (accepts short ID like "WEB-81D" or numeric ID)
    /// Ignored issues won't reopen when new events arrive
    pub async fn ignore_issue(&self, issue_id: &str) -> Result<Value> {
        self.update_issue_status(issue_id, "ignored").await
    }

    /// Unresolve an issue (set back to unresolved)
    pub async fn unresolve_issue(&self, issue_id: &str) -> Result<Value> {
        self.update_issue_status(issue_id, "unresolved").await
    }

    /// Snooze an issue for a given duration in minutes
    /// Sets the issue to ignored with an ignoreDuration, so it reopens after the period
    pub async fn snooze_issue(&self, issue_id: &str, duration_minutes: u64) -> Result<Value> {
        let numeric_id = self.resolve_numeric_id(issue_id).await?;

        self.put(
            &format!("/issues/{}/", numeric_id),
            &serde_json::json!({
                "status": "ignored",
                "statusDetails": {"ignoreDuration": duration_minutes}
            }),
        )
        .await
    }

    /// Resolve a short ID (e.g. "WEB-81D") to a numeric ID, or pass through numeric IDs
    async fn resolve_numeric_id(&self, issue_id: &str) -> Result<String> {
        if issue_id.chars().any(|c| !c.is_ascii_digit()) {
            let issue = self.get_issue(issue_id).await?;
            issue["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Could not get numeric ID for issue {}", issue_id))
                .map(|s| s.to_string())
        } else {
            Ok(issue_id.to_string())
        }
    }

    /// Update issue status (helper for resolve/ignore)
    async fn update_issue_status(&self, issue_id: &str, status: &str) -> Result<Value> {
        let numeric_id = self.resolve_numeric_id(issue_id).await?;

        self.put(
            &format!("/issues/{}/", numeric_id),
            &serde_json::json!({"status": status}),
        )
        .await
    }

    /// List monitors for the organization
    pub async fn list_monitors(&self, environment: Option<&str>) -> Result<Value> {
        let mut endpoint = format!("/organizations/{}/monitors/", self.organization);
        if let Some(env) = environment {
            endpoint.push_str(&format!("?environment={}", urlencoding::encode(env)));
        }
        self.get(&endpoint).await
    }

    /// Get monitor details by slug
    pub async fn get_monitor(&self, monitor_slug: &str) -> Result<Value> {
        self.get(&format!(
            "/organizations/{}/monitors/{}/",
            self.organization, monitor_slug
        ))
        .await
    }

    /// List check-ins for a monitor
    pub async fn list_monitor_checkins(
        &self,
        monitor_slug: &str,
        limit: Option<u32>,
    ) -> Result<Value> {
        let limit = limit.unwrap_or(20);
        self.get(&format!(
            "/organizations/{}/monitors/{}/checkins/?per_page={}",
            self.organization, monitor_slug, limit
        ))
        .await
    }

    /// Delete a monitor by slug
    pub async fn delete_monitor(&self, monitor_slug: &str) -> Result<reqwest::StatusCode> {
        self.delete(&format!(
            "/organizations/{}/monitors/{}/",
            self.organization, monitor_slug
        ))
        .await
    }

    /// List internal integrations for the organization
    pub async fn list_integrations(&self) -> Result<Value> {
        self.get(&format!(
            "/organizations/{}/sentry-apps/?status=internal",
            self.organization
        ))
        .await
    }

    /// Get integration details by slug
    pub async fn get_integration(&self, slug: &str) -> Result<Value> {
        self.get(&format!("/sentry-apps/{}/", slug)).await
    }

    /// List releases for the organization
    pub async fn list_releases(&self, project: Option<&str>, limit: Option<u32>) -> Result<Value> {
        let limit = limit.unwrap_or(25);
        let mut endpoint = format!(
            "/organizations/{}/releases/?per_page={}",
            self.organization, limit
        );
        if let Some(proj) = project {
            endpoint.push_str(&format!("&project={}", urlencoding::encode(proj)));
        }
        self.get(&endpoint).await
    }

    /// Get release details by version
    pub async fn get_release(&self, version: &str) -> Result<Value> {
        self.get(&format!(
            "/organizations/{}/releases/{}/",
            self.organization,
            urlencoding::encode(version)
        ))
        .await
    }

    /// Get trace details by trace ID
    pub async fn get_trace(&self, trace_id: &str) -> Result<Value> {
        self.get(&format!(
            "/organizations/{}/events-trace/{}/",
            self.organization, trace_id
        ))
        .await
    }

    /// Get a specific event by project slug and event ID
    pub async fn get_event(&self, project_slug: &str, event_id: &str) -> Result<Value> {
        self.get(&format!(
            "/organizations/{}/events/{}:{}/",
            self.organization, project_slug, event_id
        ))
        .await
    }
}

// Endpoint building helpers for testing
#[cfg(test)]
fn monitors_endpoint(org: &str, environment: Option<&str>) -> String {
    let mut endpoint = format!("/organizations/{}/monitors/", org);
    if let Some(env) = environment {
        endpoint.push_str(&format!("?environment={}", urlencoding::encode(env)));
    }
    endpoint
}

#[cfg(test)]
fn monitor_endpoint(org: &str, slug: &str) -> String {
    format!("/organizations/{}/monitors/{}/", org, slug)
}

#[cfg(test)]
fn monitor_checkins_endpoint(org: &str, slug: &str, limit: u32) -> String {
    format!(
        "/organizations/{}/monitors/{}/checkins/?per_page={}",
        org, slug, limit
    )
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[test]
    fn test_monitors_endpoint_without_env() {
        let endpoint = monitors_endpoint("globalcomix", None);
        assert_eq!(endpoint, "/organizations/globalcomix/monitors/");
    }

    #[test]
    fn test_monitors_endpoint_with_env() {
        let endpoint = monitors_endpoint("globalcomix", Some("production"));
        assert_eq!(
            endpoint,
            "/organizations/globalcomix/monitors/?environment=production"
        );
    }

    #[test]
    fn test_monitors_endpoint_with_special_chars() {
        let endpoint = monitors_endpoint("globalcomix", Some("prod env"));
        assert_eq!(
            endpoint,
            "/organizations/globalcomix/monitors/?environment=prod%20env"
        );
    }

    #[test]
    fn test_monitor_endpoint() {
        let endpoint = monitor_endpoint("globalcomix", "daily-backup");
        assert_eq!(
            endpoint,
            "/organizations/globalcomix/monitors/daily-backup/"
        );
    }

    #[test]
    fn test_monitor_checkins_endpoint() {
        let endpoint = monitor_checkins_endpoint("globalcomix", "daily-backup", 10);
        assert_eq!(
            endpoint,
            "/organizations/globalcomix/monitors/daily-backup/checkins/?per_page=10"
        );
    }

    #[test]
    fn test_monitor_checkins_endpoint_default_limit() {
        let endpoint = monitor_checkins_endpoint("globalcomix", "my-cron", 20);
        assert_eq!(
            endpoint,
            "/organizations/globalcomix/monitors/my-cron/checkins/?per_page=20"
        );
    }

    #[tokio::test]
    async fn client_sends_auth_headers_and_reads_json() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());

        let issue = client.get_issue("ISSUE-1").await.unwrap();
        let latest = client.get_issue_latest_event("ISSUE-1").await.unwrap();
        let events = client.get_issue_events("ISSUE-1").await.unwrap();
        let event = client.get_issue_event("ISSUE-1", "event-1").await.unwrap();
        let hashes = client.get_issue_hashes("ISSUE-1").await.unwrap();
        let projects = client.list_projects().await.unwrap();

        assert_eq!(issue["id"], "1001");
        assert_eq!(latest["eventID"], "latest");
        assert_eq!(events["items"][0]["id"], "event-list");
        assert_eq!(event["eventID"], "event-1");
        assert_eq!(hashes["hashes"][0], "hash-1");
        assert_eq!(projects["projects"][0]["slug"], "web");
        assert!(
            server
                .requests()
                .iter()
                .any(|request| request.contains("authorization: bearer test-token"))
        );
    }

    #[tokio::test]
    async fn client_searches_error_events_with_encoded_parameters_fields_and_auth() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());

        let events = client
            .search_events(
                "flutter app",
                Some("user.id:762159 error.value:\"bad value\""),
                "2026-08-12T16:27:00Z",
                "2026-08-12T16:29:00+00:00",
                100,
            )
            .await
            .unwrap();

        assert_eq!(
            events,
            serde_json::json!({
                "data": [{
                    "id": "event-1",
                    "timestamp": "2026-08-12T16:28:22Z",
                    "title": "Purchase completion failed",
                    "issue": "FLUTTER-1",
                    "user.id": "762159",
                    "message": "boom"
                }],
                "meta": {"dataset": "errors"}
            })
        );

        let request = server
            .requests()
            .into_iter()
            .find(|request| request.starts_with("get /api/0/organizations/test-org/events/?"))
            .expect("event search request should be sent");
        for parameter in [
            "dataset=errors",
            "project=flutter%20app",
            "query=user.id%3a762159%20error.value%3a%22bad%20value%22",
            "start=2026-08-12t16%3a27%3a00z",
            "end=2026-08-12t16%3a29%3a00%2b00%3a00",
            "per_page=100",
            "sort=-timestamp",
            "field=id",
            "field=timestamp",
            "field=title",
            "field=issue",
            "field=user.id",
            "field=message",
        ] {
            assert!(
                request.contains(parameter),
                "missing parameter: {parameter}"
            );
        }
        assert!(request.contains("authorization: bearer test-token"));
    }

    #[tokio::test]
    async fn client_preserves_empty_event_search_result() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());

        let events = client
            .search_events(
                "flutter",
                Some("empty:true"),
                "2026-08-12T00:00:00Z",
                "2026-08-13T00:00:00Z",
                25,
            )
            .await
            .unwrap();

        assert_eq!(
            events,
            serde_json::json!({"data": [], "meta": {"dataset": "errors"}})
        );
    }

    #[tokio::test]
    async fn client_reports_event_search_api_failure_with_status_and_body() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());

        let error = client
            .search_events(
                "flutter",
                Some("fail:true"),
                "2026-08-12T00:00:00Z",
                "2026-08-13T00:00:00Z",
                10,
            )
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("HTTP 400 Bad Request - invalid explore query")
        );
    }

    #[tokio::test]
    async fn client_searches_span_rows_with_default_fields_and_auth() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());

        let request = SpanSearchRequest {
            project: "web app",
            query: Some("transaction:api/v1/account/username"),
            start: "2026-09-04T08:14:30Z",
            end: "2026-09-04T09:14:30+00:00",
            limit: 2,
            fields: None,
            sort: "-timestamp",
        };
        let spans = client.search_spans(&request).await.unwrap();

        assert_eq!(
            spans,
            serde_json::json!({
                "data": [
                    {
                        "timestamp": "2026-09-04T08:30:00Z",
                        "transaction": "api/v1/account/username",
                        "trace": "trace-1",
                        "id": "event-1",
                        "span_id": "span-1",
                        "span.op": "db",
                        "span.description": "UPDATE users",
                        "span.duration": 12.5
                    },
                    {
                        "timestamp": "2026-09-04T08:29:00Z",
                        "transaction": "api/v1/account/username",
                        "trace": "trace-2",
                        "id": "event-2",
                        "span_id": "span-2",
                        "span.op": "http.client",
                        "span.description": "GET /account/me",
                        "span.duration": 25.0
                    }
                ],
                "meta": {"dataset": "spans"}
            })
        );

        let request = server
            .requests()
            .into_iter()
            .find(|request| request.starts_with("get /api/0/organizations/test-org/events/?"))
            .expect("span search request should be sent");
        for parameter in [
            "dataset=spans",
            "project=web%20app",
            "query=transaction%3aapi%2fv1%2faccount%2fusername",
            "start=2026-09-04t08%3a14%3a30z",
            "end=2026-09-04t09%3a14%3a30%2b00%3a00",
            "per_page=2",
            "sort=-timestamp",
            "field=timestamp",
            "field=transaction",
            "field=trace",
            "field=id",
            "field=span_id",
            "field=span.op",
            "field=span.description",
            "field=span.duration",
        ] {
            assert!(
                request.contains(parameter),
                "missing parameter: {parameter}"
            );
        }
        assert!(request.contains("authorization: bearer test-token"));
    }

    #[tokio::test]
    async fn client_searches_span_rows_with_field_override_sort_and_preserves_empty_results() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());
        let fields = [
            "timestamp".to_string(),
            "trace".to_string(),
            "transaction.id".to_string(),
        ];

        let request = SpanSearchRequest {
            project: "web",
            query: Some("empty:true"),
            start: "2026-09-04T08:14:30Z",
            end: "2026-09-04T09:14:30Z",
            limit: 25,
            fields: Some(&fields),
            sort: "span.duration",
        };
        let spans = client.search_spans(&request).await.unwrap();

        assert_eq!(
            spans,
            serde_json::json!({"data": [], "meta": {"dataset": "spans"}})
        );

        let request = server
            .requests()
            .into_iter()
            .find(|request| request.starts_with("get /api/0/organizations/test-org/events/?"))
            .expect("span search request should be sent");
        for parameter in [
            "field=timestamp",
            "field=trace",
            "field=transaction.id",
            "sort=span.duration",
            "per_page=25",
        ] {
            assert!(
                request.contains(parameter),
                "missing parameter: {parameter}"
            );
        }
        assert!(!request.contains("field=span.op"));
    }

    #[tokio::test]
    async fn client_reports_span_search_api_failure_with_status_and_body() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());

        let request = SpanSearchRequest {
            project: "web",
            query: Some("fail:spans"),
            start: "2026-09-04T08:14:30Z",
            end: "2026-09-04T09:14:30Z",
            limit: 10,
            fields: None,
            sort: "-timestamp",
        };
        let error = client.search_spans(&request).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("HTTP 400 Bad Request - invalid span query")
        );
    }

    #[tokio::test]
    async fn client_updates_issue_statuses_and_snoozes_short_ids() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());

        let resolved = client.resolve_issue("ISSUE-1").await.unwrap();
        let ignored = client.ignore_issue("1001").await.unwrap();
        let unresolved = client.unresolve_issue("1001").await.unwrap();
        let snoozed = client.snooze_issue("ISSUE-1", 60).await.unwrap();

        assert_eq!(resolved["status"], "resolved");
        assert_eq!(ignored["status"], "ignored");
        assert_eq!(unresolved["status"], "unresolved");
        assert_eq!(snoozed["statusDetails"]["ignoreDuration"], 60);
        let requests = server.requests();
        assert!(requests.iter().any(|request| {
            request.starts_with("get /api/0/organizations/test-org/issues/issue-1/")
        }));
        assert!(requests.iter().any(|request| {
            request.starts_with("put /api/0/issues/1001/")
                && request.contains("\"status\":\"resolved\"")
        }));
    }

    #[tokio::test]
    async fn client_covers_monitor_release_integration_and_trace_endpoints() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());

        assert_eq!(
            client.list_issues("web", None).await.unwrap()["query"],
            "is:unresolved"
        );
        assert_eq!(
            client
                .list_issues("web", Some("assigned:me"))
                .await
                .unwrap()["query"],
            "assigned:me"
        );
        assert_eq!(
            client.list_monitors(Some("prod env")).await.unwrap()["kind"],
            "monitors"
        );
        assert_eq!(client.get_monitor("daily").await.unwrap()["slug"], "daily");
        assert_eq!(
            client
                .list_monitor_checkins("daily", Some(3))
                .await
                .unwrap()["limit"],
            3
        );
        assert_eq!(
            client.delete_monitor("daily").await.unwrap(),
            reqwest::StatusCode::NO_CONTENT
        );
        assert_eq!(
            client.list_integrations().await.unwrap()["items"][0]["slug"],
            "app"
        );
        assert_eq!(client.get_integration("app").await.unwrap()["slug"], "app");
        assert_eq!(
            client.list_releases(Some("web"), Some(5)).await.unwrap()["project"],
            "web"
        );
        assert_eq!(
            client.get_release("v1.0+build").await.unwrap()["version"],
            "v1.0+build"
        );
        assert_eq!(
            client.get_trace("trace-1").await.unwrap()["trace"],
            "trace-1"
        );
        assert_eq!(
            client.get_event("web", "event-1").await.unwrap()["eventID"],
            "event-1"
        );
    }

    #[tokio::test]
    async fn client_reports_http_errors_with_body() {
        let server = MockSentry::start(handler);
        let client = Client::for_test(server.base_url());

        let err = client.get_issue("missing").await.unwrap_err();

        assert!(
            err.to_string()
                .contains("HTTP 404 Not Found - missing issue")
        );
    }

    pub(super) fn handler(line: &str, body: &str) -> (&'static str, String) {
        if let Some(response) = issue_response(line, body) {
            return response;
        }
        if let Some(response) = monitor_response(line) {
            return response;
        }
        if let Some(response) = event_search_response(line) {
            return response;
        }
        if let Some(response) = org_resource_response(line) {
            return response;
        }
        ("500 Internal Server Error", format!("unexpected: {line}"))
    }

    fn issue_response(line: &str, body: &str) -> Option<(&'static str, String)> {
        if line.starts_with("GET /api/0/organizations/test-org/issues/missing/") {
            return Some(("404 Not Found", "missing issue".to_string()));
        }
        if line.starts_with("GET /api/0/organizations/test-org/issues/ISSUE-1/") {
            return Some((
                "200 OK",
                serde_json::json!({"id": "1001", "shortId": "ISSUE-1"}).to_string(),
            ));
        }
        issue_event_response(line, body)
    }

    fn issue_event_response(line: &str, body: &str) -> Option<(&'static str, String)> {
        if line.starts_with("GET /api/0/issues/ISSUE-1/events/latest/") {
            return Some((
                "200 OK",
                serde_json::json!({"eventID": "latest"}).to_string(),
            ));
        }
        if line.starts_with("GET /api/0/issues/ISSUE-1/events/event-1/") {
            return Some((
                "200 OK",
                serde_json::json!({"eventID": "event-1"}).to_string(),
            ));
        }
        if line.starts_with("GET /api/0/issues/ISSUE-1/events/") {
            return Some((
                "200 OK",
                serde_json::json!({"items": [{"id": "event-list"}]}).to_string(),
            ));
        }
        if line.starts_with("GET /api/0/issues/ISSUE-1/hashes/") {
            return Some((
                "200 OK",
                serde_json::json!({"hashes": ["hash-1"]}).to_string(),
            ));
        }
        issue_org_response(line, body)
    }

    fn issue_org_response(line: &str, body: &str) -> Option<(&'static str, String)> {
        if line.starts_with("GET /api/0/organizations/test-org/projects/") {
            return Some((
                "200 OK",
                serde_json::json!({"projects": [{"slug": "web"}]}).to_string(),
            ));
        }
        issue_mutation_response(line, body)
    }

    fn issue_mutation_response(line: &str, body: &str) -> Option<(&'static str, String)> {
        if line.starts_with("GET /api/0/projects/test-org/web/issues/") {
            return Some(("200 OK", list_issues_response(line)));
        }
        if line.starts_with("PUT /api/0/issues/1001/") {
            return Some(("200 OK", status_response(body)));
        }
        None
    }

    fn monitor_response(line: &str) -> Option<(&'static str, String)> {
        if line.starts_with("GET /api/0/organizations/test-org/monitors/?") {
            return Some((
                "200 OK",
                serde_json::json!({"kind": "monitors"}).to_string(),
            ));
        }
        if line.starts_with("GET /api/0/organizations/test-org/monitors/daily/checkins/") {
            return Some(("200 OK", serde_json::json!({"limit": 3}).to_string()));
        }
        if line.starts_with("GET /api/0/organizations/test-org/monitors/daily/") {
            return Some(("200 OK", serde_json::json!({"slug": "daily"}).to_string()));
        }
        if line.starts_with("DELETE /api/0/organizations/test-org/monitors/daily/") {
            return Some(("204 No Content", String::new()));
        }
        None
    }

    fn event_search_response(line: &str) -> Option<(&'static str, String)> {
        if !line.starts_with("GET /api/0/organizations/test-org/events/?") {
            return None;
        }
        if line.contains("dataset=spans") && !line.contains("field=count%28%29") {
            return span_search_response(line);
        }
        if line.contains("dataset=spans") {
            return Some(super::performance::test_response(line));
        }
        if line.contains("query=fail%3Atrue") {
            return Some(("400 Bad Request", "invalid explore query".to_string()));
        }
        if line.contains("query=empty%3Atrue") {
            return Some((
                "200 OK",
                serde_json::json!({"data": [], "meta": {"dataset": "errors"}}).to_string(),
            ));
        }
        Some((
            "200 OK",
            serde_json::json!({
                "data": [{
                    "id": "event-1",
                    "timestamp": "2026-08-12T16:28:22Z",
                    "title": "Purchase completion failed",
                    "issue": "FLUTTER-1",
                    "user.id": "762159",
                    "message": "boom"
                }],
                "meta": {"dataset": "errors"}
            })
            .to_string(),
        ))
    }

    fn span_search_response(line: &str) -> Option<(&'static str, String)> {
        if line.contains("query=fail%3Aspans") {
            return Some(("400 Bad Request", "invalid span query".to_string()));
        }
        if line.contains("query=empty%3Atrue") {
            return Some((
                "200 OK",
                serde_json::json!({"data": [], "meta": {"dataset": "spans"}}).to_string(),
            ));
        }
        Some((
            "200 OK",
            serde_json::json!({
                "data": [
                    {
                        "timestamp": "2026-09-04T08:30:00Z",
                        "transaction": "api/v1/account/username",
                        "trace": "trace-1",
                        "id": "event-1",
                        "span_id": "span-1",
                        "span.op": "db",
                        "span.description": "UPDATE users",
                        "span.duration": 12.5
                    },
                    {
                        "timestamp": "2026-09-04T08:29:00Z",
                        "transaction": "api/v1/account/username",
                        "trace": "trace-2",
                        "id": "event-2",
                        "span_id": "span-2",
                        "span.op": "http.client",
                        "span.description": "GET /account/me",
                        "span.duration": 25.0
                    }
                ],
                "meta": {"dataset": "spans"}
            })
            .to_string(),
        ))
    }

    fn org_resource_response(line: &str) -> Option<(&'static str, String)> {
        if line.starts_with("GET /api/0/organizations/test-org/sentry-apps/?status=internal") {
            return Some((
                "200 OK",
                serde_json::json!({"items": [{"slug": "app"}]}).to_string(),
            ));
        }
        if line.starts_with("GET /api/0/sentry-apps/app/") {
            return Some(("200 OK", serde_json::json!({"slug": "app"}).to_string()));
        }
        if line.starts_with("GET /api/0/organizations/test-org/releases/?") {
            return Some(("200 OK", release_list_response(line)));
        }
        if line.starts_with("GET /api/0/organizations/test-org/releases/v1.0%2Bbuild/") {
            return Some((
                "200 OK",
                serde_json::json!({"version": "v1.0+build"}).to_string(),
            ));
        }
        if line.starts_with("GET /api/0/organizations/test-org/events-trace/trace-1/") {
            return Some((
                "200 OK",
                serde_json::json!({"trace": "trace-1"}).to_string(),
            ));
        }
        if line.starts_with("GET /api/0/organizations/test-org/events/web:event-1/") {
            return Some((
                "200 OK",
                serde_json::json!({"eventID": "event-1"}).to_string(),
            ));
        }
        None
    }

    fn list_issues_response(line: &str) -> String {
        let query = if line.contains("assigned%3Ame") {
            "assigned:me"
        } else {
            "is:unresolved"
        };
        serde_json::json!({"query": query}).to_string()
    }

    fn release_list_response(line: &str) -> String {
        let project = if line.contains("project=web") {
            "web"
        } else {
            ""
        };
        serde_json::json!({"project": project}).to_string()
    }

    fn status_response(body: &str) -> String {
        let parsed: Value = serde_json::from_str(body).unwrap();
        serde_json::json!({
            "shortId": "ISSUE-1",
            "status": parsed["status"],
            "statusDetails": parsed["statusDetails"]
        })
        .to_string()
    }

    pub(super) struct MockSentry {
        addr: SocketAddr,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl MockSentry {
        pub(super) fn start(handler: fn(&str, &str) -> (&'static str, String)) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let thread_requests = Arc::clone(&requests);
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    handle_connection(stream, &thread_requests, handler);
                }
            });
            Self { addr, requests }
        }

        pub(super) fn base_url(&self) -> String {
            format!("http://{}/api/0", self.addr)
        }

        pub(super) fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    fn handle_connection(
        mut stream: std::net::TcpStream,
        requests: &Arc<Mutex<Vec<String>>>,
        handler: fn(&str, &str) -> (&'static str, String),
    ) {
        let mut buffer = [0_u8; 16384];
        let bytes = stream.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
        let request_line = request.lines().next().unwrap_or_default().to_string();
        requests.lock().unwrap().push(request.to_lowercase());
        let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
        let (status, response_body) = handler(&request_line, body);
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        stream.write_all(response.as_bytes()).unwrap();
    }
}
