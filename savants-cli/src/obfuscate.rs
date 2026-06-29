//! Runtime query obfuscation.
//!
//! Every Cypher query passes through `q()` which reconstructs the query
//! from encrypted parts at runtime. The cleartext never exists as a
//! contiguous string in the binary's .rodata section.

const K: &[u8] = b"s4v4nts_m3m0ry_3ng1n3_2026_x9k2";

/// Decrypt an XOR'd byte slice into a String.
#[inline(always)]
pub fn d(enc: &[u8]) -> String {
    let out: Vec<u8> = enc.iter().enumerate()
        .map(|(i, &b)| b ^ K[i % K.len()])
        .collect();
    unsafe { String::from_utf8_unchecked(out) }
}

/// Encrypt a string (for generating the encrypted constants).
/// Call this once to get the byte arrays, then embed them as `const`.
pub fn e(s: &str) -> Vec<u8> {
    s.as_bytes().iter().enumerate()
        .map(|(i, &b)| b ^ K[i % K.len()])
        .collect()
}

/// Helper to print encrypted bytes as a Rust array literal.
/// Used during development to generate the constants.
pub fn print_encrypted(name: &str, query: &str) {
    let enc = e(query);
    let bytes: Vec<String> = enc.iter().map(|b| format!("0x{:02x}", b)).collect();
    println!("const {}: &[u8] = &[{}];", name, bytes.join(","));
}
