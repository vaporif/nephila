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
}

#[test]
fn redact_strips_keyvalue_pair() {
    let s = "ANTHROPIC_API_KEY=sk-secret123 something else";
    let r = redact_stderr(s);
    assert!(!r.contains("sk-secret123"), "got: {r}");
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
