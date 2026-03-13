//! HTTP client for the OpenCode API.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::time::Duration;

use crate::approval::types::PermissionReply;

/// HTTP client for all OpenCode API endpoints.
pub struct OpenCodeBridge {
    client: Client,
    base_url: String,
    password: Option<String>,
}

/// Information about an OpenCode session.
#[derive(serde::Deserialize, Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
}

impl OpenCodeBridge {
    /// Creates a new bridge client.
    ///
    /// `url` is typically "http://localhost", `port` is the OpenCode server port.
    pub fn new(url: &str, port: u16, password: Option<String>) -> Self {
        let base_url = format!("{}:{}", url.trim_end_matches('/'), port);
        OpenCodeBridge {
            client: Client::new(),
            base_url,
            password,
        }
    }

    /// Returns the base URL (e.g. "http://localhost:4096").
    pub fn get_base_url(&self) -> &str {
        &self.base_url
    }

    /// Builds the Authorization header value for Basic auth.
    fn auth_header(&self) -> Option<String> {
        self.password.as_ref().map(|pw| {
            let credentials = format!("opencode:{}", pw);
            format!("Basic {}", STANDARD.encode(credentials))
        })
    }

    /// Performs a GET request and deserializes the JSON response.
    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.get(&url);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }
        let resp = req
            .send()
            .await
            .with_context(|| friendly_connection_error(&self.base_url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenCode API error {}: {}", status, body);
        }
        let result = resp
            .json::<T>()
            .await
            .with_context(|| format!("Failed to deserialize response from {}", path))?;
        Ok(result)
    }

    /// Performs a POST request with JSON body and deserializes the JSON response.
    async fn post_json_response<T: DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.post(&url).json(&body);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }
        let resp = req
            .send()
            .await
            .with_context(|| friendly_connection_error(&self.base_url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenCode API error {}: {}", status, body);
        }
        let result = resp
            .json::<T>()
            .await
            .with_context(|| format!("Failed to deserialize response from {}", path))?;
        Ok(result)
    }

    /// Performs a POST request with JSON body.
    async fn post_json(&self, path: &str, body: serde_json::Value) -> Result<()> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.post(&url).json(&body);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }
        let resp = req
            .send()
            .await
            .with_context(|| friendly_connection_error(&self.base_url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenCode API error {}: {}", status, body);
        }
        Ok(())
    }

    /// Checks if OpenCode is healthy. Returns Ok(true) if healthy, Ok(false) otherwise.
    pub async fn health_check(&self) -> Result<bool> {
        let result: Result<serde_json::Value> = self.get_json("/global/health").await;
        match result {
            Ok(value) => Ok(value.get("healthy").and_then(|v| v.as_bool()).unwrap_or(false)),
            Err(_) => Ok(false),
        }
    }

    /// Lists recent sessions.
    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        self.get_json::<Vec<SessionInfo>>("/session?limit=10&roots=true")
            .await
    }

    /// Creates a new session.
    pub async fn create_session(&self) -> Result<SessionInfo> {
        self.post_json_response::<SessionInfo>("/session", json!({}))
            .await
    }

    /// Sends a text message to a session asynchronously.
    pub async fn send_message(&self, session_id: &str, text: &str) -> Result<()> {
        let path = format!("/session/{}/prompt_async", session_id);
        let body = json!({
            "parts": [{ "type": "text", "text": text }]
        });
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.client.post(&url).json(&body);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }
        let resp = req
            .send()
            .await
            .with_context(|| friendly_connection_error(&self.base_url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            anyhow::bail!("send_message failed: {}", status);
        }
        Ok(())
    }

    /// Checks if OpenCode is reachable. Never panics.
    pub async fn is_connected(&self) -> bool {
        let url = format!("{}/global/health", self.base_url);
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap_or_else(|_| Client::new());
        let mut req = client.get(&url);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }
        req.send().await.map(|r| r.status().is_success()).unwrap_or(false)
    }

    /// Replies to a permission request.
    pub async fn reply_permission(
        &self,
        id: &str,
        reply: PermissionReply,
        message: Option<&str>,
    ) -> Result<()> {
        let path = format!("/permission/{}/reply", id);
        let mut body = json!({"reply": reply});
        if let Some(msg) = message {
            body["message"] = json!(msg);
        }
        self.post_json(&path, body).await
    }

    /// Replies to a question request with answers.
    pub async fn reply_question(&self, id: &str, answers: Vec<Vec<String>>) -> Result<()> {
        let path = format!("/question/{}/reply", id);
        self.post_json(&path, json!({"answers": answers})).await
    }

    /// Rejects (dismisses) a question request.
    pub async fn reject_question(&self, id: &str) -> Result<()> {
        let path = format!("/question/{}/reject", id);
        self.post_json(&path, json!({})).await
    }
}

fn friendly_connection_error(base_url: &str) -> String {
    format!(
        "Cannot connect to OpenCode at {}. Make sure OpenCode is running with 'opencode web' or 'opencode --port <port>'",
        base_url
    )
}
