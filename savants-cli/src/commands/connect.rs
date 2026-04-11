//! OAuth 2.0 Device Authorization Grant flow for savants.cloud.
//!
//! Same pattern as `tailscale up`, `gh auth login`, `az login`:
//!
//! 1. CLI requests a device code from savants.cloud
//! 2. User opens a URL in their browser and signs in
//! 3. CLI polls until the browser session completes
//! 4. Device token is saved to ~/.savants/config.toml
//!
//! The user never copies a token. The CLI handles everything.

use colored::*;
use crate::config::{State, CLOUD_ENDPOINT};

/// Run the device authorization flow.
pub async fn run() {
    println!("{}", "Connecting to savants.cloud...".bold());
    println!();

    let state = State::load();

    // Check if already connected
    if state.is_cloud_authenticated() {
        let org = state.cloud_org.as_deref().unwrap_or("unknown");
        println!("  {} Already connected to org: {}", "●".green(), org.cyan());
        println!("  Run {} to disconnect.", "savants disconnect".dimmed());
        return;
    }

    // Step 1: Request device code
    println!("Requesting device authorization...");
    println!();

    // In production, this would be an HTTP POST to:
    //   POST {CLOUD_ENDPOINT}/auth/device/code
    //   → { device_code, user_code, verification_uri, interval, expires_in }
    //
    // For now, show what the flow WILL look like when savants.cloud is built.
    // The actual HTTP calls are stubbed until the cloud backend exists.

    let verification_url = format!("{}/auth/device", CLOUD_ENDPOINT);

    println!("To authenticate, visit:");
    println!();
    println!("    {}", verification_url.cyan().bold().underline());
    println!();
    println!("Waiting for authentication...");
    println!("{}", "(This will complete automatically once you sign in)".dimmed());
    println!();

    // Step 2: Poll for completion
    // In production, this polls:
    //   POST {CLOUD_ENDPOINT}/auth/device/token
    //   { device_code, grant_type: "urn:ietf:params:oauth:grant-type:device_code" }
    //
    // Response states:
    //   - authorization_pending → keep polling
    //   - slow_down → increase interval
    //   - access_denied → user denied
    //   - expired_token → device code expired, restart flow
    //   - success → { access_token, org, device_id }

    // STUB: In the real implementation, this would loop polling the token endpoint.
    // For now, we print what happens and exit gracefully.
    println!("{}", "savants.cloud is not yet live. This flow will work when the cloud tier launches.".yellow());
    println!();
    println!("What will happen:");
    println!("  1. You open the URL above in your browser");
    println!("  2. Sign in with Google, GitHub, or your company's SSO");
    println!("  3. The CLI automatically receives a device token");
    println!("  4. Your local graphs start syncing to savants.cloud");
    println!("  5. Your team can see cross-cluster and cross-repo queries");
    println!();
    println!("Want to be notified when savants.cloud launches?");
    println!("  Visit {}", "https://savants.dev/cloud".cyan());
}

/// Disconnect from savants.cloud.
pub fn disconnect() {
    let mut state = State::load();

    if !state.is_cloud_authenticated() {
        println!("Not connected to savants.cloud.");
        return;
    }

    state.cloud_device_token = None;
    state.cloud_device_id = None;
    state.cloud_org = None;
    if let Err(e) = state.save() {
        eprintln!("{}: {}", "Error".red(), e);
        return;
    }

    println!("{}", "Disconnected from savants.cloud.".green());
    println!("Your local graphs are unaffected. Federation sync has stopped.");
}
