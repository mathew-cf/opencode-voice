//! HTTP client for the OpenCode API.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::Client;
use serde_json::json;
use std::time::Duration;

use crate::approval::types::PermissionReply;

/// HTTP client for all OpenCode API endpoints.
pub struct OpenCodeBridge {
    client: Client,
    base_url: String,
    password: Option<String>,
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
            let credentials = format!(":{}", pw);
            format!("Basic {}", STANDARD.encode(credentials))
        })
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

    /// Injects text into OpenCode's prompt.
    pub async fn append_prompt(
        &self,
        text: &str,
        directory: Option<&str>,
        workspace: Option<&str>,
    ) -> Result<()> {
        let mut url = format!("{}/tui/append-prompt", self.base_url);
        let mut params = Vec::new();
        if let Some(dir) = directory {
            params.push(format!("directory={}", urlencoding_encode(dir)));
        }
        if let Some(ws) = workspace {
            params.push(format!("workspace={}", urlencoding_encode(ws)));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let mut req = self.client.post(&url).json(&json!({"text": text}));
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }
        let resp = req
            .send()
            .await
            .with_context(|| friendly_connection_error(&self.base_url))?;
        if !resp.status().is_success() {
            let status = resp.status();
            anyhow::bail!("append_prompt failed: {}", status);
        }
        Ok(())
    }

    /// Submits the OpenCode prompt.
    pub async fn submit_prompt(&self) -> Result<()> {
        self.post_json("/tui/submit-prompt", json!({})).await
    }

    /// Checks if OpenCode is reachable. Never panics.
    pub async fn is_connected(&self) -> bool {
        let url = format!("{}/", self.base_url);
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap_or_else(|_| Client::new());
        let mut req = client.get(&url);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }
        req.send().await.is_ok()
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
        "Cannot connect to OpenCode at {}. Make sure OpenCode is running with --port flag: opencode --port <port>",
        base_url
    )
}

fn urlencoding_encode(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                vec![c]
            } else {
                format!("%{:02X}", c as u32).chars().collect::<Vec<_>>()
            }
        })
        .collect()
}
