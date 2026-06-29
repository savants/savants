//! Simple log template extraction (drain3-like).
//!
//! Masks variable tokens (IPs, numbers, hex, UUIDs) with placeholder
//! tokens so that structurally identical log lines collapse into a
//! single template. Returns a (template_text, template_hash) pair
//! where the hash is a hex-encoded SHA-256 of the template text.

use regex::Regex;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

static RE_UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
        .unwrap()
});

static RE_IP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap()
});

static RE_HEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b0x[0-9a-fA-F]{8,}\b|\b[0-9a-fA-F]{9,}\b").unwrap()
});

static RE_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{4,}\b").unwrap()
});

/// Extract a template from a raw log line.
///
/// Replaces UUIDs with `<UUID>`, IP addresses with `<IP>`,
/// hex strings (>= 8 chars) with `<HEX>`, and numbers (> 3 digits) with `<NUM>`.
///
/// Returns `(template_text, template_hash)` where hash is hex SHA-256.
pub fn extract_template(line: &str) -> (String, String) {
    // Order matters: UUID before HEX (UUIDs contain hex chars)
    let t = RE_UUID.replace_all(line, "<UUID>");
    let t = RE_IP.replace_all(&t, "<IP>");
    let t = RE_HEX.replace_all(&t, "<HEX>");
    let t = RE_NUM.replace_all(&t, "<NUM>");

    let template = t.to_string();
    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(template.as_bytes());
        let result = hasher.finalize();
        format!("{:x}", result)[..16].to_string() // 16-char hex prefix
    };

    (template, hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ip_replacement() {
        let (t, _) = extract_template("Connection from 192.168.1.1 refused");
        assert!(t.contains("<IP>"));
        assert!(!t.contains("192.168.1.1"));
    }

    #[test]
    fn test_uuid_replacement() {
        let (t, _) = extract_template("Request 550e8400-e29b-41d4-a716-446655440000 failed");
        assert!(t.contains("<UUID>"));
    }

    #[test]
    fn test_number_replacement() {
        let (t, _) = extract_template("Process 12345 used 98765 bytes");
        assert!(t.contains("<NUM>"));
        assert!(!t.contains("12345"));
    }

    #[test]
    fn test_hex_replacement() {
        let (t, _) = extract_template("address 0xdeadbeef01 fault");
        assert!(t.contains("<HEX>"));
    }

    #[test]
    fn test_deterministic_hash() {
        let (_, h1) = extract_template("error at 192.168.1.1 port 8080");
        let (_, h2) = extract_template("error at 10.0.0.1 port 9090");
        // Same template after masking → same hash
        assert_eq!(h1, h2);
    }
}
