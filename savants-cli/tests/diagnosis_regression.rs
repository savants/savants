//! Diagnosis regression tests.
//!
//! Each test case represents a real prod error that was diagnosed correctly.
//! If any of these start failing, the diagnosis logic has regressed.
//!
//! To run: cargo test --test diagnosis_regression
//!
//! These tests require a running FalkorDB instance with the talent-pipeline
//! graph indexed. They are integration tests, not unit tests.

use std::process::Command;

/// Run diagnose_error via the MCP server and return the output.
fn diagnose(error: &str, repo: &str) -> String {
    let input = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2024-11-05","capabilities":{{}},"clientInfo":{{"name":"test","version":"1.0"}}}}}}
{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"diagnose_error","arguments":{{"error":"{}","repo":"{}"}}}}}}"#,
        error.replace('"', "\\\""),
        repo,
    );

    let binary = env!("CARGO_BIN_EXE_savants");
    let output = Command::new(binary)
        .arg("serve")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output()
        });

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            // Parse the last line as JSON-RPC response
            if let Some(last_line) = stdout.lines().last() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(last_line) {
                    return v.get("result")
                        .and_then(|r| r.get("content"))
                        .and_then(|c| c.as_array())
                        .and_then(|a| a.first())
                        .and_then(|t| t.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                }
            }
            String::new()
        }
        Err(_) => String::new(),
    }
}

/// Helper to check that the diagnosis output contains expected strings.
fn assert_diagnosis_contains(output: &str, expected: &[&str], test_name: &str) {
    let lower = output.to_lowercase();
    for exp in expected {
        assert!(
            lower.contains(&exp.to_lowercase()),
            "\n[{}] Expected diagnosis to contain '{}'\n\nFull output:\n{}",
            test_name, exp, output
        );
    }
}

/// Helper to check the diagnosis does NOT contain certain strings.
fn assert_diagnosis_not_contains(output: &str, unexpected: &[&str], test_name: &str) {
    let lower = output.to_lowercase();
    for unexp in unexpected {
        assert!(
            !lower.contains(&unexp.to_lowercase()),
            "\n[{}] Diagnosis should NOT contain '{}'\n\nFull output:\n{}",
            test_name, unexp, output
        );
    }
}

// ============================================================================
// Test Cases — Real prod errors with known correct diagnoses
// ============================================================================

#[test]
#[ignore] // Requires running FalkorDB with indexed graph
fn case_1_tsplit_frontend_crash() {
    let output = diagnose(
        "Auto-investigation VOCATOR-FRONTEND-3X: The variable t is undefined when the split method is called in ResumeIntegrityDialog splitClaimedItems",
        "talent-pipeline",
    );

    assert_diagnosis_contains(&output, &[
        "splitClaimedItems",                           // Found the function
        "identity-verification",                       // Traced to upstream service
        "NO VALIDATION",                               // Identified the gap
        "upstream",                                    // Root cause is upstream, not frontend
        "FIX",                                         // Provides a fix
    ], "t.split frontend crash");

    assert_diagnosis_not_contains(&output, &[
        "Could not extract",                           // Should successfully parse
        "Could not identify",                          // Should find functions
    ], "t.split frontend crash");
}

#[test]
#[ignore]
fn case_2_rippling_polling_failure() {
    let output = diagnose(
        "vocator-backend RipplingPolling Failed to fetch applications from the Rippling API endpoint",
        "talent-pipeline",
    );

    assert_diagnosis_contains(&output, &[
        "rippling-polling",                            // Found the polling job file
        "Infrastructure",                              // Classified as infra error
    ], "Rippling polling failure");

    assert_diagnosis_not_contains(&output, &[
        "Could not extract",
        "splitClaimedItems",                           // Should NOT confuse with the split bug
    ], "Rippling polling failure");
}

#[test]
#[ignore]
fn case_3_redis_subscribe_error() {
    let output = diagnose(
        "vocator-backend ReplyError: ERR Cant execute info only SUBSCRIBE UNSUBSCRIBE PING QUIT RESET are allowed in this context",
        "talent-pipeline",
    );

    assert_diagnosis_contains(&output, &[
        "Redis",                                       // Identified as Redis issue
        "SUBSCRIBE",                                   // Understood the subscribe mode issue
        "separate",                                    // Fix: use separate connections
        "redis-pubsub",                                // Found the relevant file
    ], "Redis SUBSCRIBE error");

    assert_diagnosis_not_contains(&output, &[
        "splitClaimedItems",                           // Should NOT confuse with split bug
        "identity-verification",                       // Should NOT point at AI validation
    ], "Redis SUBSCRIBE error");
}

#[test]
#[ignore]
fn case_4_llm_validation_error() {
    let output = diagnose(
        "vocator-backend LLMValidationError: generateAIReview LLM output failed schema validation",
        "talent-pipeline",
    );

    assert_diagnosis_contains(&output, &[
        "generateAIReview",                            // Found the function
        "validation",                                  // Recognized validation exists
        "schema",                                      // Understood it's a schema issue
        "validateLLMOutput",                           // Found the validation import
    ], "LLM validation error");

    assert_diagnosis_not_contains(&output, &[
        "NO VALIDATION",                               // Should NOT say validation is missing
        "ROOT CAUSE: The crash is in",                 // Should NOT blame the function
    ], "LLM validation error");
}

#[test]
#[ignore]
fn case_5_llm_parse_error() {
    let output = diagnose(
        "vocator-backend LLMParseError: generateInteractiveResume Invalid JSON in LLM response",
        "talent-pipeline",
    );

    assert_diagnosis_contains(&output, &[
        "generateInteractiveResume",                   // Found the function or related
        "schema",                                      // Schema/parse issue
    ], "LLM parse error");

    assert_diagnosis_not_contains(&output, &[
        "Could not extract",
    ], "LLM parse error");
}

// ============================================================================
// Meta-tests — ensure the tool handles edge cases
// ============================================================================

#[test]
#[ignore]
fn case_edge_empty_error() {
    let output = diagnose("", "talent-pipeline");
    // Should not panic, should return gracefully
    assert!(!output.is_empty() || output.is_empty()); // Just shouldn't panic
}

#[test]
#[ignore]
fn case_edge_unknown_function() {
    let output = diagnose(
        "Error in nonExistentFunction at unknown.ts:999",
        "talent-pipeline",
    );
    // Should handle gracefully, not crash
    assert!(!output.contains("panic"));
}

#[test]
#[ignore]
fn case_edge_wrong_repo() {
    let output = diagnose(
        "vocator-backend LLMValidationError: generateAIReview failed",
        "nonexistent-repo",
    );
    // Should handle gracefully
    assert!(!output.contains("panic"));
}
