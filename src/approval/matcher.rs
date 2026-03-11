//! Voice command matching for approval handling.
//!
//! Matches transcribed text against permission approval patterns and question answers.

use crate::approval::types::{PermissionReply, QuestionRequest};

/// Result of matching a voice command against an approval context.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchResult {
    /// Matched a permission reply command.
    PermissionReply {
        reply: PermissionReply,
        message: Option<String>,
    },
    /// Matched a question answer.
    QuestionAnswer { answers: Vec<Vec<String>> },
    /// Matched a question rejection.
    QuestionReject,
    /// No match found.
    NoMatch,
}

/// Normalizes text: lowercase, trim, strip trailing punctuation, collapse whitespace.
pub fn normalize(text: &str) -> String {
    let lower = text.to_lowercase();
    let trimmed = lower.trim();
    // Strip trailing punctuation
    let stripped =
        trimmed.trim_end_matches(|c: char| matches!(c, '.' | '!' | '?' | ',' | ';' | ':'));
    // Collapse internal whitespace
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Patterns that result in a "once" permission grant.
const ONCE_PATTERNS: &[&str] = &[
    "allow",
    "allow once",
    "allow it",
    "allow this",
    "yes",
    "yeah",
    "yep",
    "ok",
    "okay",
    "sure",
    "proceed",
    "go ahead",
    "do it",
    "approve",
    "approve once",
    "approve it",
    "approve this",
    "accept",
    "run it",
    "go for it",
    "permit",
    "execute",
];

/// Patterns that result in an "always" permission grant.
const ALWAYS_PATTERNS: &[&str] = &["always", "always allow", "trust", "trust it", "allow all"];

/// Patterns that result in a permission rejection.
const REJECT_PATTERNS: &[&str] = &[
    "reject",
    "deny",
    "no",
    "nope",
    "cancel",
    "stop",
    "don't",
    "do not",
    "refuse",
    "block",
    "skip",
    "decline",
    "dismiss",
    "not allowed",
    "nah",
    "don't do it",
];

/// Rejection prefixes that can be followed by a message.
const REJECT_PREFIXES: &[&str] = &["no", "nope", "reject", "deny", "cancel", "don't", "refuse"];

/// Question rejection phrases.
const QUESTION_REJECT_PATTERNS: &[&str] = &[
    "skip",
    "dismiss",
    "cancel",
    "reject",
    "none",
    "never mind",
    "nevermind",
];

/// Number word to 1-based index mapping.
fn parse_number_word(word: &str) -> Option<usize> {
    match word {
        "one" | "first" | "1" => Some(1),
        "two" | "second" | "2" => Some(2),
        "three" | "third" | "3" => Some(3),
        "four" | "fourth" | "4" => Some(4),
        "five" | "fifth" | "5" => Some(5),
        "six" | "sixth" | "6" => Some(6),
        "seven" | "seventh" | "7" => Some(7),
        "eight" | "eighth" | "8" => Some(8),
        "nine" | "ninth" | "9" => Some(9),
        "ten" | "tenth" | "10" => Some(10),
        _ => None,
    }
}

/// Matches a voice command against permission approval patterns.
///
/// Returns the appropriate MatchResult based on which pattern matches.
pub fn match_permission_command(text: &str) -> MatchResult {
    let normalized = normalize(text);

    // Check always patterns first (more specific than once)
    for pattern in ALWAYS_PATTERNS {
        if normalized == *pattern {
            return MatchResult::PermissionReply {
                reply: PermissionReply::Always,
                message: None,
            };
        }
    }

    // Check once patterns
    for pattern in ONCE_PATTERNS {
        if normalized == *pattern {
            return MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                message: None,
            };
        }
    }

    // Check exact reject patterns
    for pattern in REJECT_PATTERNS {
        if normalized == *pattern {
            return MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                message: None,
            };
        }
    }

    // Check reject with message: "no, <message>" or "reject, <message>" etc.
    for prefix in REJECT_PREFIXES {
        if let Some(after) = normalized.strip_prefix(prefix) {
            let after = after.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
            if !after.is_empty() {
                return MatchResult::PermissionReply {
                    reply: PermissionReply::Reject,
                    message: Some(after.to_string()),
                };
            }
        }
    }

    MatchResult::NoMatch
}

/// Matches a voice command against question options.
///
/// Returns QuestionReject, QuestionAnswer, or NoMatch.
pub fn match_question_answer(text: &str, question: &QuestionRequest) -> MatchResult {
    let normalized = normalize(text);

    // Check question rejection phrases first
    for pattern in QUESTION_REJECT_PATTERNS {
        if normalized == *pattern {
            return MatchResult::QuestionReject;
        }
    }

    // Process each question in the request
    let mut all_answers: Vec<Vec<String>> = Vec::new();

    for q in &question.questions {
        let options = &q.options;

        // 1. Exact label match
        let exact = options.iter().find(|o| normalize(&o.label) == normalized);
        if let Some(opt) = exact {
            all_answers.push(vec![opt.label.clone()]);
            continue;
        }

        // 2. Label-in-text match
        let contains = options
            .iter()
            .find(|o| normalized.contains(&normalize(&o.label)));
        if let Some(opt) = contains {
            all_answers.push(vec![opt.label.clone()]);
            continue;
        }

        // 3. Numeric match: "option 1", "one", "first", etc.
        // Check "option N" pattern
        let numeric = if let Some(after_option) = normalized.strip_prefix("option ") {
            parse_number_word(after_option.trim())
        } else {
            // Check bare number word
            parse_number_word(&normalized)
        };

        if let Some(idx) = numeric {
            if idx >= 1 && idx <= options.len() {
                all_answers.push(vec![options[idx - 1].label.clone()]);
                continue;
            }
        }

        // 4. Custom answer fallback (if question allows custom responses)
        if q.custom {
            all_answers.push(vec![text.trim().to_string()]);
            continue;
        }

        // No match for this question
        return MatchResult::NoMatch;
    }

    if all_answers.is_empty() {
        MatchResult::NoMatch
    } else {
        MatchResult::QuestionAnswer {
            answers: all_answers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::types::{QuestionInfo, QuestionOption, QuestionRequest};

    fn make_question(options: &[&str], custom: bool) -> QuestionRequest {
        QuestionRequest {
            id: "test-id".to_string(),
            questions: vec![QuestionInfo {
                question: "What do you want to do?".to_string(),
                options: options
                    .iter()
                    .map(|&l| QuestionOption {
                        label: l.to_string(),
                    })
                    .collect(),
                custom,
            }],
        }
    }

    // Permission tests — Once patterns (22)
    #[test]
    fn test_once_allow() {
        assert!(matches!(
            match_permission_command("allow"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }
    #[test]
    fn test_once_yes() {
        assert!(matches!(
            match_permission_command("yes"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }
    #[test]
    fn test_once_ok() {
        assert!(matches!(
            match_permission_command("ok"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }
    #[test]
    fn test_once_okay() {
        assert!(matches!(
            match_permission_command("okay"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }
    #[test]
    fn test_once_sure() {
        assert!(matches!(
            match_permission_command("sure"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }
    #[test]
    fn test_once_proceed() {
        assert!(matches!(
            match_permission_command("proceed"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }
    #[test]
    fn test_once_approve() {
        assert!(matches!(
            match_permission_command("approve"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }
    #[test]
    fn test_once_execute() {
        assert!(matches!(
            match_permission_command("execute"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }
    #[test]
    fn test_once_accept() {
        assert!(matches!(
            match_permission_command("accept"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }
    #[test]
    fn test_once_yeah() {
        assert!(matches!(
            match_permission_command("yeah"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }
    #[test]
    fn test_once_with_punctuation() {
        // "yes." should normalize to "yes" and match once
        assert!(matches!(
            match_permission_command("yes."),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    // Always patterns (5)
    #[test]
    fn test_always_always() {
        assert!(matches!(
            match_permission_command("always"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Always,
                ..
            }
        ));
    }
    #[test]
    fn test_always_always_allow() {
        assert!(matches!(
            match_permission_command("always allow"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Always,
                ..
            }
        ));
    }
    #[test]
    fn test_always_trust() {
        assert!(matches!(
            match_permission_command("trust"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Always,
                ..
            }
        ));
    }
    #[test]
    fn test_always_trust_it() {
        assert!(matches!(
            match_permission_command("trust it"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Always,
                ..
            }
        ));
    }
    #[test]
    fn test_always_allow_all() {
        assert!(matches!(
            match_permission_command("allow all"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Always,
                ..
            }
        ));
    }

    // Reject patterns (16)
    #[test]
    fn test_reject_no() {
        assert!(matches!(
            match_permission_command("no"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }
    #[test]
    fn test_reject_reject() {
        assert!(matches!(
            match_permission_command("reject"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }
    #[test]
    fn test_reject_deny() {
        assert!(matches!(
            match_permission_command("deny"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }
    #[test]
    fn test_reject_cancel() {
        assert!(matches!(
            match_permission_command("cancel"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }
    #[test]
    fn test_reject_nope() {
        assert!(matches!(
            match_permission_command("nope"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }

    // Reject with message
    #[test]
    fn test_reject_with_message() {
        let result = match_permission_command("no, try something else");
        match result {
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                message,
            } => {
                assert_eq!(message, Some("try something else".to_string()));
            }
            _ => panic!("Expected reject with message"),
        }
    }

    // NoMatch
    #[test]
    fn test_no_match() {
        assert_eq!(
            match_permission_command("hello world"),
            MatchResult::NoMatch
        );
    }
    #[test]
    fn test_no_match_empty() {
        assert_eq!(match_permission_command(""), MatchResult::NoMatch);
    }

    // Question tests
    #[test]
    fn test_question_exact_label() {
        let q = make_question(&["Continue", "Cancel", "Retry"], true);
        let result = match_question_answer("continue", &q);
        assert!(matches!(result, MatchResult::QuestionAnswer { .. }));
    }

    #[test]
    fn test_question_numeric_option_1() {
        let q = make_question(&["Yes", "No"], true);
        let result = match_question_answer("option 1", &q);
        assert!(matches!(result, MatchResult::QuestionAnswer { .. }));
    }

    #[test]
    fn test_question_numeric_word_first() {
        let q = make_question(&["Alpha", "Beta"], true);
        let result = match_question_answer("first", &q);
        assert!(
            matches!(result, MatchResult::QuestionAnswer { answers } if answers[0] == vec!["Alpha"])
        );
    }

    #[test]
    fn test_question_numeric_word_one() {
        let q = make_question(&["Alpha", "Beta"], true);
        let result = match_question_answer("one", &q);
        assert!(
            matches!(result, MatchResult::QuestionAnswer { answers } if answers[0] == vec!["Alpha"])
        );
    }

    #[test]
    fn test_question_reject_skip() {
        let q = make_question(&["Yes", "No"], true);
        assert_eq!(
            match_question_answer("skip", &q),
            MatchResult::QuestionReject
        );
    }

    #[test]
    fn test_question_reject_dismiss() {
        let q = make_question(&["Yes", "No"], true);
        assert_eq!(
            match_question_answer("dismiss", &q),
            MatchResult::QuestionReject
        );
    }

    #[test]
    fn test_question_custom_answer() {
        let q = make_question(&["Yes", "No"], true);
        let result = match_question_answer("do something custom", &q);
        assert!(matches!(result, MatchResult::QuestionAnswer { .. }));
    }

    #[test]
    fn test_question_no_match_no_custom() {
        let q = make_question(&["Yes", "No"], false);
        let result = match_question_answer("do something custom", &q);
        assert_eq!(result, MatchResult::NoMatch);
    }

    #[test]
    fn test_normalize_punctuation() {
        assert_eq!(normalize("yes."), "yes");
        assert_eq!(normalize("Allow!"), "allow");
        assert_eq!(normalize("  ok  "), "ok");
    }

    // Additional coverage tests
    #[test]
    fn test_once_all_patterns() {
        let expected_once = [
            "allow",
            "allow once",
            "allow it",
            "allow this",
            "yes",
            "yeah",
            "yep",
            "ok",
            "okay",
            "sure",
            "proceed",
            "go ahead",
            "do it",
            "approve",
            "approve once",
            "approve it",
            "approve this",
            "accept",
            "run it",
            "go for it",
            "permit",
            "execute",
        ];
        assert_eq!(
            expected_once.len(),
            22,
            "Must have exactly 22 once patterns"
        );
        for pattern in &expected_once {
            assert!(
                matches!(
                    match_permission_command(pattern),
                    MatchResult::PermissionReply {
                        reply: PermissionReply::Once,
                        ..
                    }
                ),
                "Pattern '{}' should match Once",
                pattern
            );
        }
    }

    #[test]
    fn test_always_all_patterns() {
        let expected_always = ["always", "always allow", "trust", "trust it", "allow all"];
        assert_eq!(
            expected_always.len(),
            5,
            "Must have exactly 5 always patterns"
        );
        for pattern in &expected_always {
            assert!(
                matches!(
                    match_permission_command(pattern),
                    MatchResult::PermissionReply {
                        reply: PermissionReply::Always,
                        ..
                    }
                ),
                "Pattern '{}' should match Always",
                pattern
            );
        }
    }

    #[test]
    fn test_reject_all_patterns() {
        let expected_reject = [
            "reject",
            "deny",
            "no",
            "nope",
            "cancel",
            "stop",
            "don't",
            "do not",
            "refuse",
            "block",
            "skip",
            "decline",
            "dismiss",
            "not allowed",
            "nah",
            "don't do it",
        ];
        assert_eq!(
            expected_reject.len(),
            16,
            "Must have exactly 16 reject patterns"
        );
        for pattern in &expected_reject {
            assert!(
                matches!(
                    match_permission_command(pattern),
                    MatchResult::PermissionReply {
                        reply: PermissionReply::Reject,
                        ..
                    }
                ),
                "Pattern '{}' should match Reject",
                pattern
            );
        }
    }

    #[test]
    fn test_always_takes_priority_over_once() {
        // "always allow" contains "allow" (once) but should match Always
        assert!(matches!(
            match_permission_command("always allow"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Always,
                ..
            }
        ));
    }

    #[test]
    fn test_not_allowed_exact_reject() {
        // "not allowed" starts with "no" but should be exact reject, not reject-with-message
        let result = match_permission_command("not allowed");
        match result {
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                message,
            } => {
                assert_eq!(message, None, "'not allowed' should have no message");
            }
            _ => panic!("Expected reject"),
        }
    }

    #[test]
    fn test_question_label_in_text() {
        let q = make_question(&["Continue", "Cancel"], true);
        // "I want to continue" contains "continue"
        let result = match_question_answer("I want to continue", &q);
        assert!(
            matches!(result, MatchResult::QuestionAnswer { answers } if answers[0] == vec!["Continue"])
        );
    }

    #[test]
    fn test_question_numeric_option_2() {
        let q = make_question(&["Alpha", "Beta", "Gamma"], true);
        let result = match_question_answer("option 2", &q);
        assert!(
            matches!(result, MatchResult::QuestionAnswer { answers } if answers[0] == vec!["Beta"])
        );
    }

    #[test]
    fn test_question_numeric_word_second() {
        let q = make_question(&["Alpha", "Beta"], true);
        let result = match_question_answer("second", &q);
        assert!(
            matches!(result, MatchResult::QuestionAnswer { answers } if answers[0] == vec!["Beta"])
        );
    }

    #[test]
    fn test_question_reject_never_mind() {
        let q = make_question(&["Yes", "No"], true);
        assert_eq!(
            match_question_answer("never mind", &q),
            MatchResult::QuestionReject
        );
    }

    #[test]
    fn test_question_empty_questions_no_match() {
        let q = QuestionRequest {
            id: "test".to_string(),
            questions: vec![],
        };
        assert_eq!(match_question_answer("yes", &q), MatchResult::NoMatch);
    }

    // --- Additional tests added to expand coverage ---

    // normalize: extra whitespace collapsing
    #[test]
    fn test_normalize_extra_whitespace() {
        assert_eq!(normalize("  hello   world  "), "hello world");
    }

    // normalize: mixed case
    #[test]
    fn test_normalize_mixed_case() {
        assert_eq!(normalize("ALLOW"), "allow");
        assert_eq!(normalize("AlLoW OnCe"), "allow once");
    }

    // normalize: multiple trailing punctuation chars
    #[test]
    fn test_normalize_multiple_trailing_punctuation() {
        assert_eq!(normalize("yes!?"), "yes");
        assert_eq!(normalize("ok..."), "ok");
    }

    // normalize: internal punctuation is preserved (only trailing stripped)
    #[test]
    fn test_normalize_internal_punctuation_preserved() {
        // Commas in the middle are not stripped
        let result = normalize("no, try again");
        assert_eq!(result, "no, try again");
    }

    // Once patterns: remaining ones not individually tested above
    #[test]
    fn test_once_yep() {
        assert!(matches!(
            match_permission_command("yep"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    #[test]
    fn test_once_go_ahead() {
        assert!(matches!(
            match_permission_command("go ahead"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    #[test]
    fn test_once_do_it() {
        assert!(matches!(
            match_permission_command("do it"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    #[test]
    fn test_once_run_it() {
        assert!(matches!(
            match_permission_command("run it"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    #[test]
    fn test_once_go_for_it() {
        assert!(matches!(
            match_permission_command("go for it"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    #[test]
    fn test_once_permit() {
        assert!(matches!(
            match_permission_command("permit"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    #[test]
    fn test_once_allow_once() {
        assert!(matches!(
            match_permission_command("allow once"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    #[test]
    fn test_once_allow_it() {
        assert!(matches!(
            match_permission_command("allow it"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    #[test]
    fn test_once_allow_this() {
        assert!(matches!(
            match_permission_command("allow this"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    #[test]
    fn test_once_approve_once() {
        assert!(matches!(
            match_permission_command("approve once"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    #[test]
    fn test_once_approve_it() {
        assert!(matches!(
            match_permission_command("approve it"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    #[test]
    fn test_once_approve_this() {
        assert!(matches!(
            match_permission_command("approve this"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    // Reject patterns: remaining ones not individually tested above
    #[test]
    fn test_reject_stop() {
        assert!(matches!(
            match_permission_command("stop"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }

    #[test]
    fn test_reject_dont() {
        assert!(matches!(
            match_permission_command("don't"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }

    #[test]
    fn test_reject_do_not() {
        assert!(matches!(
            match_permission_command("do not"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }

    #[test]
    fn test_reject_refuse() {
        assert!(matches!(
            match_permission_command("refuse"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }

    #[test]
    fn test_reject_block() {
        assert!(matches!(
            match_permission_command("block"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }

    #[test]
    fn test_reject_skip() {
        assert!(matches!(
            match_permission_command("skip"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }

    #[test]
    fn test_reject_decline() {
        assert!(matches!(
            match_permission_command("decline"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }

    #[test]
    fn test_reject_dismiss() {
        assert!(matches!(
            match_permission_command("dismiss"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }

    #[test]
    fn test_reject_nah() {
        assert!(matches!(
            match_permission_command("nah"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }

    #[test]
    fn test_reject_dont_do_it() {
        assert!(matches!(
            match_permission_command("don't do it"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }

    // Reject with message: various prefixes
    #[test]
    fn test_reject_with_message_deny_prefix() {
        let result = match_permission_command("deny, not safe");
        match result {
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                message,
            } => {
                assert_eq!(message, Some("not safe".to_string()));
            }
            _ => panic!("Expected reject with message"),
        }
    }

    #[test]
    fn test_reject_with_message_cancel_prefix() {
        let result = match_permission_command("cancel, wrong command");
        match result {
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                message,
            } => {
                assert_eq!(message, Some("wrong command".to_string()));
            }
            _ => panic!("Expected reject with message"),
        }
    }

    #[test]
    fn test_reject_with_message_no_space_separator() {
        // "no try again" — "no" is a reject prefix, "try again" is the message
        let result = match_permission_command("no try again");
        match result {
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                message,
            } => {
                assert_eq!(message, Some("try again".to_string()));
            }
            _ => panic!("Expected reject with message"),
        }
    }

    // Case-insensitive matching via normalize
    #[test]
    fn test_once_case_insensitive() {
        assert!(matches!(
            match_permission_command("YES"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
        assert!(matches!(
            match_permission_command("Allow"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Once,
                ..
            }
        ));
    }

    #[test]
    fn test_always_case_insensitive() {
        assert!(matches!(
            match_permission_command("ALWAYS"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Always,
                ..
            }
        ));
        assert!(matches!(
            match_permission_command("Trust"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Always,
                ..
            }
        ));
    }

    #[test]
    fn test_reject_case_insensitive() {
        assert!(matches!(
            match_permission_command("NO"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
        assert!(matches!(
            match_permission_command("Deny"),
            MatchResult::PermissionReply {
                reply: PermissionReply::Reject,
                ..
            }
        ));
    }

    // Question: reject with cancel
    #[test]
    fn test_question_reject_cancel() {
        let q = make_question(&["Yes", "No"], true);
        assert_eq!(
            match_question_answer("cancel", &q),
            MatchResult::QuestionReject
        );
    }

    // Question: reject with nevermind (no space)
    #[test]
    fn test_question_reject_nevermind() {
        let q = make_question(&["Yes", "No"], true);
        assert_eq!(
            match_question_answer("nevermind", &q),
            MatchResult::QuestionReject
        );
    }

    // Question: reject with "none"
    #[test]
    fn test_question_reject_none() {
        let q = make_question(&["Yes", "No"], true);
        assert_eq!(
            match_question_answer("none", &q),
            MatchResult::QuestionReject
        );
    }

    // Question: numeric "two" / "second" selects second option
    #[test]
    fn test_question_numeric_word_two() {
        let q = make_question(&["Alpha", "Beta", "Gamma"], true);
        let result = match_question_answer("two", &q);
        assert!(
            matches!(result, MatchResult::QuestionAnswer { answers } if answers[0] == vec!["Beta"])
        );
    }

    // Question: "option 3" selects third option
    #[test]
    fn test_question_numeric_option_3() {
        let q = make_question(&["Alpha", "Beta", "Gamma"], true);
        let result = match_question_answer("option 3", &q);
        assert!(
            matches!(result, MatchResult::QuestionAnswer { answers } if answers[0] == vec!["Gamma"])
        );
    }

    // Question: out-of-range numeric falls through to custom
    #[test]
    fn test_question_numeric_out_of_range_with_custom() {
        let q = make_question(&["Alpha", "Beta"], true);
        // "option 5" is out of range (only 2 options), falls through to custom
        let result = match_question_answer("option 5", &q);
        assert!(matches!(result, MatchResult::QuestionAnswer { .. }));
    }

    // Question: out-of-range numeric without custom → NoMatch
    #[test]
    fn test_question_numeric_out_of_range_no_custom() {
        let q = make_question(&["Alpha", "Beta"], false);
        // "option 5" is out of range, no custom → NoMatch
        let result = match_question_answer("option 5", &q);
        assert_eq!(result, MatchResult::NoMatch);
    }

    // Question: exact label match is case-insensitive (via normalize)
    #[test]
    fn test_question_exact_label_case_insensitive() {
        let q = make_question(&["Continue", "Cancel", "Retry"], false);
        let result = match_question_answer("CONTINUE", &q);
        assert!(
            matches!(result, MatchResult::QuestionAnswer { answers } if answers[0] == vec!["Continue"])
        );
    }

    // Question: custom answer preserves original text (not normalized)
    #[test]
    fn test_question_custom_answer_preserves_text() {
        let q = make_question(&["Yes", "No"], true);
        let result = match_question_answer("  My Custom Answer  ", &q);
        match result {
            MatchResult::QuestionAnswer { answers } => {
                // Custom answer uses text.trim()
                assert_eq!(answers[0], vec!["My Custom Answer"]);
            }
            _ => panic!("Expected QuestionAnswer"),
        }
    }

    // NoMatch: random text
    #[test]
    fn test_no_match_random_text() {
        assert_eq!(
            match_permission_command("the quick brown fox"),
            MatchResult::NoMatch
        );
    }

    // NoMatch: whitespace only
    #[test]
    fn test_no_match_whitespace_only() {
        assert_eq!(match_permission_command("   "), MatchResult::NoMatch);
    }
}
