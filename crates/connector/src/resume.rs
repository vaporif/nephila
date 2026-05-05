//! Helpers for `ClaudeCodeSession::resume` — fallback heuristics for the
//! `claude --resume <id>` → `claude --session-id <id>` path.
//!
//! The "resume target not found" stderr signature is observed empirically.
//! TODO: tighten once we have a verbatim capture of real `claude --resume`
//! stderr against a missing id. Until then we keep the matcher loose
//! (case-insensitive substring scan over a small set of known phrasings)
//! to avoid false negatives that would leave a fresh session-id stuck in
//! a hard error.

/// Phrases real `claude` is observed (or expected) to print on stderr when
/// `--resume <id>` is passed an id with no on-disk session record.
///
/// All comparisons are case-insensitive. A match on any one substring counts
/// as "resume target not found".
const RESUME_NOT_FOUND_PATTERNS: &[&str] = &[
    "session not found",
    "session_not_found",
    "no conversation matching",
    "no such session",
    "could not find session",
    "no session with id",
];

/// Returns `true` if `stderr` looks like a "resume target not found" message.
///
/// The matcher is intentionally loose — see module docs.
#[must_use]
pub fn is_resume_not_found(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    RESUME_NOT_FOUND_PATTERNS.iter().any(|p| lower.contains(*p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_session_not_found() {
        assert!(is_resume_not_found("error: Session not found: abc-def"));
    }

    #[test]
    fn matches_no_conversation() {
        assert!(is_resume_not_found(
            "ERROR: No conversation matching id 123"
        ));
    }

    #[test]
    fn case_insensitive() {
        assert!(is_resume_not_found("SESSION NOT FOUND"));
        assert!(is_resume_not_found("session not found"));
    }

    #[test]
    fn unrelated_stderr_does_not_match() {
        assert!(!is_resume_not_found(""));
        assert!(!is_resume_not_found("permission denied"));
        assert!(!is_resume_not_found("config error: missing field"));
    }
}
