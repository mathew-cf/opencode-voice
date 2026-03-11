//! FIFO queue for pending approval items (permissions and questions).

use std::collections::VecDeque;

use crate::approval::types::{PendingApproval, PermissionRequest, QuestionRequest};

/// FIFO queue for pending permission and question approvals.
pub struct ApprovalQueue {
    items: VecDeque<PendingApproval>,
}

impl ApprovalQueue {
    pub fn new() -> Self {
        ApprovalQueue {
            items: VecDeque::new(),
        }
    }

    /// Returns the number of pending items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns true if there are pending items.
    pub fn has_pending(&self) -> bool {
        !self.items.is_empty()
    }

    /// Returns a reference to the front item without removing it.
    pub fn peek(&self) -> Option<&PendingApproval> {
        self.items.front()
    }

    /// Adds a permission request to the back of the queue.
    pub fn add_permission(&mut self, request: PermissionRequest) {
        self.items.push_back(PendingApproval::Permission(request));
    }

    /// Adds a question request to the back of the queue.
    pub fn add_question(&mut self, request: QuestionRequest) {
        self.items.push_back(PendingApproval::Question(request));
    }

    /// Removes the item with the given request ID. Returns true if found and removed.
    pub fn remove(&mut self, request_id: &str) -> bool {
        if let Some(pos) = self.items.iter().position(|item| item.id() == request_id) {
            self.items.remove(pos);
            true
        } else {
            false
        }
    }
}

impl Default for ApprovalQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::types::{PermissionRequest, QuestionRequest};

    fn make_permission(id: &str) -> PermissionRequest {
        PermissionRequest {
            id: id.to_string(),
            permission: "bash".to_string(),
            metadata: serde_json::Value::Null,
        }
    }

    fn make_question(id: &str) -> QuestionRequest {
        QuestionRequest {
            id: id.to_string(),
            questions: vec![],
        }
    }

    #[test]
    fn test_new_queue_is_empty() {
        let q = ApprovalQueue::new();
        assert!(!q.has_pending());
        assert_eq!(q.len(), 0);
        assert!(q.peek().is_none());
    }

    #[test]
    fn test_add_permission_and_peek() {
        let mut q = ApprovalQueue::new();
        q.add_permission(make_permission("perm-1"));
        assert_eq!(q.len(), 1);
        assert!(q.has_pending());
        let item = q.peek().unwrap();
        assert_eq!(item.id(), "perm-1");
        assert!(matches!(item, PendingApproval::Permission(_)));
    }

    #[test]
    fn test_add_question_and_peek() {
        let mut q = ApprovalQueue::new();
        q.add_question(make_question("q-1"));
        assert_eq!(q.len(), 1);
        let item = q.peek().unwrap();
        assert_eq!(item.id(), "q-1");
        assert!(matches!(item, PendingApproval::Question(_)));
    }

    #[test]
    fn test_fifo_ordering() {
        let mut q = ApprovalQueue::new();
        q.add_permission(make_permission("first"));
        q.add_question(make_question("second"));
        q.add_permission(make_permission("third"));

        // Peek should return first
        assert_eq!(q.peek().unwrap().id(), "first");
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn test_remove_found() {
        let mut q = ApprovalQueue::new();
        q.add_permission(make_permission("perm-1"));
        q.add_question(make_question("q-1"));

        let removed = q.remove("perm-1");
        assert!(removed);
        assert_eq!(q.len(), 1);
        assert_eq!(q.peek().unwrap().id(), "q-1");
    }

    #[test]
    fn test_remove_not_found() {
        let mut q = ApprovalQueue::new();
        q.add_permission(make_permission("perm-1"));

        let removed = q.remove("nonexistent");
        assert!(!removed);
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn test_remove_middle_item() {
        let mut q = ApprovalQueue::new();
        q.add_permission(make_permission("a"));
        q.add_permission(make_permission("b"));
        q.add_permission(make_permission("c"));

        q.remove("b");
        assert_eq!(q.len(), 2);
        assert_eq!(q.peek().unwrap().id(), "a");
    }

    #[test]
    fn test_has_pending_tracks_state() {
        let mut q = ApprovalQueue::new();
        assert!(!q.has_pending());

        q.add_permission(make_permission("x"));
        assert!(q.has_pending());
    }

    // --- Additional tests added to expand coverage ---

    #[test]
    fn test_default_creates_empty_queue() {
        let q = ApprovalQueue::default();
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn test_remove_last_item_leaves_empty() {
        let mut q = ApprovalQueue::new();
        q.add_permission(make_permission("only"));
        let removed = q.remove("only");
        assert!(removed);
        assert_eq!(q.len(), 0);
        assert!(q.peek().is_none());
    }

    #[test]
    fn test_remove_preserves_fifo_order() {
        let mut q = ApprovalQueue::new();
        q.add_permission(make_permission("a"));
        q.add_question(make_question("b"));
        q.add_permission(make_permission("c"));

        // Remove middle item
        q.remove("b");

        // Remaining items should be in original order
        assert_eq!(q.len(), 2);
        assert_eq!(q.peek().unwrap().id(), "a");
    }

    #[test]
    fn test_peek_does_not_remove() {
        let mut q = ApprovalQueue::new();
        q.add_permission(make_permission("p1"));
        q.add_permission(make_permission("p2"));

        // Peek multiple times — should always return same item
        assert_eq!(q.peek().unwrap().id(), "p1");
        assert_eq!(q.peek().unwrap().id(), "p1");
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn test_insertion_order_preserved() {
        let mut q = ApprovalQueue::new();
        q.add_permission(make_permission("first"));
        q.add_question(make_question("second"));
        q.add_permission(make_permission("third"));

        assert_eq!(q.len(), 3);
        assert_eq!(q.peek().unwrap().id(), "first");
    }

    #[test]
    fn test_len_increments_correctly() {
        let mut q = ApprovalQueue::new();
        assert_eq!(q.len(), 0);
        q.add_permission(make_permission("a"));
        assert_eq!(q.len(), 1);
        q.add_question(make_question("b"));
        assert_eq!(q.len(), 2);
        q.add_permission(make_permission("c"));
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn test_remove_not_found_does_not_change_len() {
        let mut q = ApprovalQueue::new();
        q.add_permission(make_permission("a"));
        q.add_permission(make_permission("b"));

        let removed = q.remove("nonexistent");
        assert!(!removed);
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn test_mixed_permission_and_question_types() {
        let mut q = ApprovalQueue::new();
        q.add_permission(make_permission("perm"));
        q.add_question(make_question("quest"));

        let first = q.peek().unwrap();
        assert!(matches!(first, PendingApproval::Permission(_)));
        assert_eq!(first.id(), "perm");

        q.remove("perm");
        let second = q.peek().unwrap();
        assert!(matches!(second, PendingApproval::Question(_)));
        assert_eq!(second.id(), "quest");
    }
}
