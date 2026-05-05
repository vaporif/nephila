use nephila_connector::redact_stderr;

#[test]
fn redact_strips_anthropic_key() {
    let s = "auth failed: sk-ant-api03-1234567890abcdef1234567890abcdef";
    let r = redact_stderr(s);
    assert!(!r.contains("sk-ant-api03"), "got: {r}");
    assert!(r.contains("<redacted>"), "got: {r}");
}

#[test]
fn redact_strips_bearer_token() {
    let s = "Authorization: Bearer eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9";
    let r = redact_stderr(s);
    assert!(!r.contains("eyJ0eXAi"), "got: {r}");
    assert!(r.contains("<redacted>"), "got: {r}");
}

#[test]
fn redact_strips_keyvalue_pair() {
    let s = "ANTHROPIC_API_KEY=sk-secret123 something else";
    let r = redact_stderr(s);
    assert!(!r.contains("sk-secret123"), "got: {r}");
    assert!(r.contains("<redacted>"), "got: {r}");
}

#[test]
fn redact_keeps_file_paths() {
    let s = "failed to read /home/user/project/file.rs: permission denied";
    let r = redact_stderr(s);
    assert!(
        r.contains("/home/user/project/file.rs"),
        "should keep paths; got: {r}"
    );
}

#[test]
fn redact_keeps_long_hashes() {
    // blake3/sha256 digests embedded in error messages must survive — they're
    // diagnostic, not credentials.
    let s = "blob hash mismatch: a3f5e8c9b1d2f4a6b8c0e2f4a6b8d0c2e4f6a8b0d2";
    let r = redact_stderr(s);
    assert!(
        r.contains("a3f5e8c9b1d2f4a6b8c0e2f4a6b8d0c2e4f6a8b0d2"),
        "should keep long hashes; got: {r}"
    );
}

#[test]
fn redact_strips_github_pat() {
    let s = "auth failed: ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let r = redact_stderr(s);
    assert!(!r.contains("ghp_abcdefghijklmnopqrstuvwxyz"), "got: {r}");
    assert!(r.contains("<redacted>"), "got: {r}");
}

#[test]
fn redact_handles_multiline_independently() {
    let s = "line 1: sk-ant-api03-1234567890abcdef1234567890abcdef\nline 2: ok\nline 3: Bearer eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9";
    let r = redact_stderr(s);
    assert!(!r.contains("sk-ant-api03"), "got: {r}");
    assert!(!r.contains("eyJ0eXAi"), "got: {r}");
    assert!(r.contains("<redacted>"), "got: {r}");
    assert!(
        r.contains("line 2: ok"),
        "innocuous lines must survive: {r}"
    );
}

#[test]
fn redact_handles_empty() {
    assert_eq!(redact_stderr(""), "");
}

#[test]
fn redact_keeps_short_sk_prefix() {
    // `{20,}` quantifier — short prefix must not match.
    let s = "sk-shortenough";
    let r = redact_stderr(s);
    assert_eq!(r, s, "short sk- prefix should not redact: {r}");
}
