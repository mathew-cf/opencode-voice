//! Approval system types: permission requests, question requests, and queue entries.

use serde::{Deserialize, Serialize};

/// A pending permission request from OpenCode.
#[derive(Debug, Clone, Deserialize)]
pub struct PermissionRequest {
    pub id: String,
    pub permission: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// How to reply to a permission request.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionReply {
    Once,
    Always,
    Reject,
}

/// A single option within a question.
#[derive(Debug, Clone, Deserialize)]
pub struct QuestionOption {
    pub label: String,
}

/// A single question within a QuestionRequest.
#[derive(Debug, Clone, Deserialize)]
pub struct QuestionInfo {
    pub question: String,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default = "default_true")]
    pub custom: bool,
}

fn default_true() -> bool {
    true
}

/// A pending question request from OpenCode.
#[derive(Debug, Clone, Deserialize)]
pub struct QuestionRequest {
    pub id: String,
    #[serde(default)]
    pub questions: Vec<QuestionInfo>,
}

/// A pending approval item — either a permission or a question.
#[derive(Debug, Clone)]
pub enum PendingApproval {
    Permission(PermissionRequest),
    Question(QuestionRequest),
}

impl PendingApproval {
    /// Returns the request ID for this approval item.
    pub fn id(&self) -> &str {
        match self {
            PendingApproval::Permission(r) => &r.id,
            PendingApproval::Question(r) => &r.id,
        }
    }
}
