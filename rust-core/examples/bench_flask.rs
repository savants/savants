//! Benchmark: parse all of flask with the Rust hot path.
//!
//! Run: cargo run --release --example bench_flask -- /tmp/profile-repos/flask

use std::time::Instant;
use savants_core::CodePropertyGraphBuilder;

fn main() {
    let repo = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/profile-repos/flask".to_string());

    println!("Parsing repo: {}", repo);
    let builder = CodePropertyGraphBuilder::new(&repo);

    // Cold start
    let start = Instant::now();
    let stats = builder.build();
    let elapsed = start.elapsed();

    println!();
    println!("=== Rust hot path ===");
    println!("  files:     {}", stats.files);
    println!("  functions: {}", stats.functions);
    println!("  classes:   {}", stats.classes);
    println!("  calls:     {}", stats.calls);
    println!("  failed:    {}", stats.failed);
    println!("  elapsed:   {:.3}s", elapsed.as_secs_f64());
    println!(
        "  rate:      {:.0} files/sec",
        stats.files as f64 / elapsed.as_secs_f64().max(0.001)
    );
}
