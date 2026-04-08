use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;

const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_SECS: u64 = 1;

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
    pub fn new(organization: &str, auth_token: &str) -> Result<Self> {
        let http = reqwest::Client::builder().build()?;

        Ok(Self {
            http,
            base_url: "https://sentry.io/api/0".to_string(),
            auth_token: auth_token.to_string(),
            organization: organization.to_string(),
        })
    }

    async fn send(&self, method: reqwest::Method, endpoint: &str) -> Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url, endpoint);
        let resp = self
            .send_with_retry(|| {
                self.http
                    .request(method.clone(), &url)
                    .header("Authorization", format!("Bearer {}", self.auth_token))
                    .header("Content-Type", "application/json")
                    .send()
            })
            .await
            .context("Failed to send request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("HTTP {} - {}", status, body);
        }

        Ok(resp)
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
                self.http
                    .put(&url)
                    .header("Authorization", format!("Bearer {}", self.auth_token))
                    .header("Content-Type", "application/json")
                    .json(body)
                    .send()
            })
            .await
            .context("Failed to send request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("HTTP {} - {}", status, body);
        }

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
        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            match make_request().await {
                Ok(resp) => return Ok(resp),
                Err(err) => {
                    if attempt < MAX_RETRIES && is_connection_timeout(&err) {
                        let delay = INITIAL_BACKOFF_SECS * 2u64.pow(attempt);
                        eprintln!(
                            "Connection timeout, retrying ({}/{})...",
                            attempt + 1,
                            MAX_RETRIES
                        );
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                        last_error = Some(err);
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        // This should only be reached if all retries failed
        Err(last_error.expect("should have an error after retries"))
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
mod tests {
    use super::*;

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
}
