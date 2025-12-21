use anyhow::{Context, Result};
use serde_json::Value;

pub struct Client {
    http: reqwest::Client,
    base_url: String,
    auth_token: String,
    organization: String,
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

    async fn get(&self, endpoint: &str) -> Result<Value> {
        let url = format!("{}{}", self.base_url, endpoint);

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .header("Content-Type", "application/json")
            .send()
            .await
            .context("Failed to send request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("HTTP {} - {}", status, body);
        }

        resp.json().await.context("Failed to parse JSON response")
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
}
