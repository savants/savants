//! Connect to external services: Slack, savants.cloud.
//!
//! Slack: auto-detects the Slack desktop app and extracts session
//! credentials. Zero copy-paste, zero browser DevTools.
//!
//! Cloud: OAuth 2.0 Device Authorization Grant flow (like `tailscale up`).

use colored::*;
use crate::config::{State, CLOUD_ENDPOINT};
use std::path::PathBuf;

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

/// Extract Slack xoxc token directly from the Slack desktop app's local storage.
/// Works on Linux and macOS — reads from LevelDB files on disk.
fn extract_slack_token() -> Option<(String, String)> {
    let home = dirs::home_dir()?;
    let paths = vec![
        home.join(".config/Slack/Local Storage/leveldb"),                           // Linux
        home.join("Library/Application Support/Slack/Local Storage/leveldb"),       // macOS
    ];

    let ldb_dir = paths.into_iter().find(|p| p.is_dir())?;

    let mut entries: Vec<_> = std::fs::read_dir(&ldb_dir).ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.ends_with(".ldb") || name.ends_with(".log")
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let token_re = regex::Regex::new(r"xoxc-[0-9]+-[0-9]+-[0-9]+-[0-9a-f]{64}").unwrap();
    let name_re = regex::Regex::new(r#""name":"([^"]+)""#).unwrap();

    let mut token: Option<String> = None;
    let mut team_name = String::from("unknown");

    for entry in &entries {
        let data = match std::fs::read(entry.path()) {
            Ok(d) => d,
            Err(_) => continue,
        };

        // Search for xoxc token by scanning bytes
        let text = String::from_utf8_lossy(&data);

        // Find token: scan raw bytes, skip non-printable gaps (LevelDB encoding)
        if token.is_none() {
            for i in 0..data.len().saturating_sub(5) {
                if &data[i..i+5] == b"xoxc-" {
                    let mut tok_bytes = Vec::new();
                    let mut j = i;
                    let mut gap = 0;
                    while j < (i + 300).min(data.len()) {
                        let b = data[j];
                        if b.is_ascii_alphanumeric() || b == b'-' {
                            tok_bytes.push(b);
                            gap = 0;
                        } else {
                            gap += 1;
                            if gap > 5 { break; }
                        }
                        j += 1;
                    }
                    let raw = String::from_utf8_lossy(&tok_bytes).to_string();
                    if let Some(m) = token_re.find(&raw) {
                        token = Some(m.as_str().to_string());
                        break;
                    }
                }
            }
        }

        // Find team name from localConfig_v2 JSON
        if let Some(config_start) = text.find("localConfig_v2") {
            if let Some(m) = name_re.captures(&text[config_start..]) {
                team_name = m[1].to_string();
            }
        }

        if token.is_some() { break; }
    }

    token.map(|t| (t, team_name))
}

/// Connect Slack for alerts and interactive RCA.
pub fn slack(webhook: Option<String>, bot_token: Option<String>, user_token: Option<String>, cookie: Option<String>, channel: Option<String>) {
    println!("{}", "Connecting Slack...".bold());
    println!();

    let savants_home = dirs::home_dir().unwrap_or_default().join(".savants");
    let config_path = savants_home.join("slack.toml");

    if webhook.is_none() && bot_token.is_none() && user_token.is_none() {
        // Auto-detect: try to extract token from Slack desktop app
        println!("  {} Looking for Slack desktop app...", "🔍");
        match extract_slack_token() {
            Some((token, team_name)) => {
                println!("  {} Found token for \"{}\"", "✅".green(), team_name.cyan());
                println!();

                // Fetch channels
                println!("  {} Fetching your channels...", "⏳");
                let client = reqwest::blocking::Client::new();
                let resp = client.get("https://slack.com/api/conversations.list")
                    .query(&[("types", "public_channel,private_channel"), ("limit", "100"), ("exclude_archived", "true")])
                    .header("Authorization", format!("Bearer {}", token))
                    .timeout(std::time::Duration::from_secs(10))
                    .send();

                let mut channels: Vec<(String, String)> = vec![];
                if let Ok(resp) = resp {
                    if let Ok(data) = resp.json::<serde_json::Value>() {
                        if let Some(ch_list) = data.get("channels").and_then(|c| c.as_array()) {
                            for ch in ch_list {
                                let id = ch.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                let is_member = ch.get("is_member").and_then(|v| v.as_bool()).unwrap_or(false);
                                if is_member && !id.is_empty() {
                                    channels.push((id.to_string(), name.to_string()));
                                }
                            }
                        }
                        if !data.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                            let err = data.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
                            if err == "invalid_auth" {
                                println!("  {} Token needs a cookie (macOS Keychain restriction)", "⚠️".yellow());
                                println!();
                                println!("  The Slack desktop token requires a session cookie that's");
                                println!("  locked in the macOS Keychain. Quick fix:");
                                println!();
                                println!("  {} Open {} in your browser", "1.".bold(), "app.slack.com".cyan());
                                println!("  {} Open DevTools → Console (F12)", "2.".bold());
                                println!("  {} Paste this:", "3.".bold());
                                println!();
                                println!("     {}", "copy(document.cookie.match(/d=([^;]+)/)[1])".dimmed());
                                println!();
                                println!("  {} Paste the cookie value here:", "4.".bold());
                                print!("     d=");
                                use std::io::Write;
                                let _ = std::io::stdout().flush();

                                let mut cookie_input = String::new();
                                if std::io::stdin().read_line(&mut cookie_input).is_ok() {
                                    let cookie_val = cookie_input.trim().to_string();
                                    if !cookie_val.is_empty() {
                                        // Retry with cookie
                                        let resp2 = client.get("https://slack.com/api/conversations.list")
                                            .query(&[("types", "public_channel,private_channel"), ("limit", "100"), ("exclude_archived", "true")])
                                            .header("Authorization", format!("Bearer {}", token))
                                            .header("Cookie", format!("d={}", cookie_val))
                                            .timeout(std::time::Duration::from_secs(10))
                                            .send();

                                        if let Ok(resp2) = resp2 {
                                            if let Ok(data2) = resp2.json::<serde_json::Value>() {
                                                if data2.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                                                    println!();
                                                    println!("  {} Authenticated!", "✅".green());

                                                    // Parse channels from this response
                                                    if let Some(ch_list) = data2.get("channels").and_then(|c| c.as_array()) {
                                                        for ch in ch_list {
                                                            let id = ch.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                                            let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                                            let is_member = ch.get("is_member").and_then(|v| v.as_bool()).unwrap_or(false);
                                                            if is_member && !id.is_empty() {
                                                                channels.push((id.to_string(), name.to_string()));
                                                            }
                                                        }
                                                    }

                                                    // Save config with cookie
                                                    if !channels.is_empty() {
                                                        println!();
                                                        for (i, (_, name)) in channels.iter().enumerate() {
                                                            println!("    {}. #{}", (i + 1).to_string().bold(), name);
                                                        }
                                                        println!();
                                                        print!("  Which channel for Savants alerts? [1-{}]: ", channels.len());
                                                        let _ = std::io::stdout().flush();

                                                        let mut ch_input = String::new();
                                                        if std::io::stdin().read_line(&mut ch_input).is_ok() {
                                                            let choice: usize = ch_input.trim().parse().unwrap_or(1);
                                                            if choice >= 1 && choice <= channels.len() {
                                                                let (ref ch_id, ref ch_name) = channels[choice - 1];
                                                                let config_content = format!(
                                                                    "user_token = \"{}\"\ncookie = \"{}\"\nchannel = \"{}\"\nworkspace = \"{}\"\n",
                                                                    token, cookie_val, ch_id, team_name
                                                                );
                                                                let _ = std::fs::write(&config_path, &config_content);
                                                                println!();
                                                                println!("  {} Connected to #{} on {}!", "✅".green(), ch_name.cyan(), team_name.cyan());
                                                                println!();
                                                                println!("  To start getting alerts:");
                                                                println!("    export SAVANTS_SLACK_USER_TOKEN=\"{}\"", token);
                                                                println!("    export SAVANTS_SLACK_COOKIE=\"{}\"", cookie_val);
                                                                println!("    export SAVANTS_SLACK_CHANNEL=\"{}\"", ch_id);
                                                                println!("    {} && {}", "savants daemon stop".dimmed(), "savants daemon start".cyan());
                                                            }
                                                        }
                                                    }
                                                    return;
                                                }
                                            }
                                        }
                                        eprintln!("  {} Cookie didn't work either. Try logging out and back in to Slack.", "✗".red());
                                    }
                                }
                            } else {
                                eprintln!("  {} Slack API error: {}", "✗".red(), err);
                            }
                            return;
                        }
                    }
                }

                if channels.is_empty() {
                    println!("  No channels found. Saving token — set channel manually:");
                    println!("  {} {}", "savants connect slack --channel".cyan(), "C0123ABC".dimmed());

                    // Save just the token
                    let config_content = format!("user_token = \"{}\"\nworkspace = \"{}\"\n", token, team_name);
                    let _ = std::fs::write(&config_path, &config_content);
                    return;
                }

                println!();
                for (i, (_, name)) in channels.iter().enumerate() {
                    println!("    {}. #{}", (i + 1).to_string().bold(), name);
                }
                println!();
                print!("  Which channel for Savants alerts? [1-{}]: ", channels.len());
                use std::io::Write;
                let _ = std::io::stdout().flush();

                let mut input = String::new();
                if std::io::stdin().read_line(&mut input).is_ok() {
                    let choice: usize = input.trim().parse().unwrap_or(1);
                    if choice >= 1 && choice <= channels.len() {
                        let (ref ch_id, ref ch_name) = channels[choice - 1];

                        // Save config
                        let config_content = format!(
                            "user_token = \"{}\"\nchannel = \"{}\"\nworkspace = \"{}\"\n",
                            token, ch_id, team_name
                        );
                        if let Err(e) = std::fs::write(&config_path, &config_content) {
                            eprintln!("{}: {}", "Error".red(), e);
                            return;
                        }

                        // Send test message
                        let test_payload = serde_json::json!({
                            "channel": ch_id,
                            "blocks": [{
                                "type": "section",
                                "text": {
                                    "type": "mrkdwn",
                                    "text": "✅ *Savants connected!*\nInfrastructure alerts will appear in this channel.\n\n`savants daemon start` to begin monitoring."
                                }
                            }]
                        });
                        let _ = client.post("https://slack.com/api/chat.postMessage")
                            .header("Authorization", format!("Bearer {}", token))
                            .json(&test_payload)
                            .timeout(std::time::Duration::from_secs(10))
                            .send();

                        println!();
                        println!("  {} Connected to #{} on {}!", "✅".green(), ch_name.cyan(), team_name.cyan());
                        println!();
                        println!("  To start getting alerts:");
                        println!("    export SAVANTS_SLACK_USER_TOKEN=\"{}\"", token);
                        println!("    export SAVANTS_SLACK_CHANNEL=\"{}\"", ch_id);
                        println!("    {} && {}", "savants daemon stop".dimmed(), "savants daemon start".cyan());
                        return;
                    }
                }
            }
            None => {
                println!("  {} Slack desktop app not found", "✗".yellow());
                println!();
                println!("  Install the Slack desktop app, or use manual setup:");
                println!("    {} {}", "savants connect slack --webhook".cyan(), "<slack-webhook-url>".dimmed());
            }
        }
        return;
    }

    // Save config
    let mut config_lines = Vec::new();
    if let Some(ref wh) = webhook {
        config_lines.push(format!("webhook_url = \"{}\"", wh));
    }
    if let Some(ref bt) = bot_token {
        config_lines.push(format!("bot_token = \"{}\"", bt));
    }
    if let Some(ref ut) = user_token {
        config_lines.push(format!("user_token = \"{}\"", ut));
    }
    if let Some(ref ck) = cookie {
        config_lines.push(format!("cookie = \"{}\"", ck));
    }
    if let Some(ref ch) = channel {
        config_lines.push(format!("channel = \"{}\"", ch));
    }
    let config_content = config_lines.join("\n") + "\n";
    if let Err(e) = std::fs::write(&config_path, &config_content) {
        eprintln!("{}: Failed to write config: {}", "Error".red(), e);
        return;
    }

    // Test the connection
    println!("  Testing connection...");
    let test_payload = serde_json::json!({
        "blocks": [
            {
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": "✅ *Savants connected!*\nAlerts will appear in this channel.\n\nTry: `savants story` to see your infrastructure narrative."
                }
            }
        ]
    });

    let client = reqwest::blocking::Client::new();
    let result = if let Some(ref wh) = webhook {
        client.post(wh).json(&test_payload).timeout(std::time::Duration::from_secs(10)).send()
    } else if let Some(ref bt) = bot_token {
        let ch = channel.as_deref().unwrap_or("#general");
        let mut payload = test_payload;
        payload["channel"] = serde_json::json!(ch);
        client.post("https://slack.com/api/chat.postMessage")
            .header("Authorization", format!("Bearer {}", bt))
            .json(&payload)
            .timeout(std::time::Duration::from_secs(10))
            .send()
    } else if let Some(ref ut) = user_token {
        let ch = channel.as_deref().unwrap_or_else(|| {
            eprintln!("{}: --channel is required with --user-token", "Error".red());
            std::process::exit(1);
        });
        let mut payload = test_payload;
        payload["channel"] = serde_json::json!(ch);
        let mut req = client.post("https://slack.com/api/chat.postMessage")
            .header("Authorization", format!("Bearer {}", ut))
            .json(&payload)
            .timeout(std::time::Duration::from_secs(10));
        if let Some(ref ck) = cookie {
            req = req.header("Cookie", format!("d={}", ck));
        }
        req.send()
    } else {
        unreachable!()
    };

    match result {
        Ok(resp) if resp.status().is_success() => {
            println!("  {} Slack connected! Check your channel for a test message.", "✅".green());
            println!();
            println!("To use with the daemon, set these env vars before starting:");
            if let Some(ref wh) = webhook {
                println!("  export SAVANTS_SLACK_WEBHOOK_URL=\"{}\"", wh);
            }
            if let Some(ref bt) = bot_token {
                println!("  export SAVANTS_SLACK_BOT_TOKEN=\"{}\"", bt);
            }
            if let Some(ref ut) = user_token {
                println!("  export SAVANTS_SLACK_USER_TOKEN=\"{}\"", ut);
            }
            if let Some(ref ck) = cookie {
                println!("  export SAVANTS_SLACK_COOKIE=\"{}\"", ck);
            }
            if let Some(ref ch) = channel {
                println!("  export SAVANTS_SLACK_CHANNEL=\"{}\"", ch);
            }
            println!();
            println!("Then restart the daemon:");
            println!("  {} && {}", "savants daemon stop".dimmed(), "savants daemon start".cyan());
        }
        Ok(resp) => {
            eprintln!("  {} Slack returned HTTP {}", "✗".red(), resp.status());
            eprintln!("  Check your webhook URL or bot token.");
        }
        Err(e) => {
            eprintln!("  {} Failed to reach Slack: {}", "✗".red(), e);
        }
    }
}

/// Auto-extract Slack token from browser.
/// Starts a tiny local HTTP server, user pastes a one-liner in Slack's browser console,
/// token flows back automatically. Zero copy-paste of credentials.
pub async fn slack_from_browser() {
    use tokio::net::TcpListener;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    println!("{}", "Savants Slack Setup".bold());
    println!();
    println!("  {} Starting local server on port 9876...", "1.".bold());

    let listener = match TcpListener::bind("127.0.0.1:9876").await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}: Cannot bind port 9876: {}", "Error".red(), e);
            return;
        }
    };

    println!("  {} Open {} in your browser (make sure you're logged in)", "2.".bold(), "app.slack.com".cyan());
    println!("  {} Open DevTools → Console (press F12)", "3.".bold());
    println!("  {} Paste this and press Enter:", "4.".bold());
    println!();

    let snippet = r#"fetch("http://localhost:9876",{method:"POST",mode:"no-cors",headers:{"Content-Type":"text/plain"},body:JSON.stringify({token:(()=>{try{let t=JSON.parse(localStorage.getItem("localConfig_v2")||"{}");return Object.values(t.teams||{})[0]?.token||""}catch{return""}})(),cookie:document.cookie.split(";").map(c=>c.trim()).find(c=>c.startsWith("d="))?.slice(2)||"",workspace:(()=>{try{let t=JSON.parse(localStorage.getItem("localConfig_v2")||"{}");return Object.values(t.teams||{})[0]?.name||""}catch{return""}})()})})"#;

    println!("  {}", snippet.dimmed());
    println!();
    println!("  {} Waiting for token...", "⏳");
    println!();

    // Wait for the browser to POST the token
    loop {
        let (mut socket, _) = match listener.accept().await {
            Ok(s) => s,
            Err(_) => continue,
        };

        let mut buf = vec![0u8; 8192];
        let n = match socket.read(&mut buf).await {
            Ok(n) => n,
            Err(_) => continue,
        };
        let request = String::from_utf8_lossy(&buf[..n]);

        // Send CORS-friendly response regardless
        let response = "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nOK";
        let _ = socket.write_all(response.as_bytes()).await;

        // Handle OPTIONS preflight
        if request.starts_with("OPTIONS") {
            continue;
        }

        // Parse POST body — it's after the \r\n\r\n
        if let Some(body_start) = request.find("\r\n\r\n") {
            let body = &request[body_start + 4..];
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(body) {
                let token = data.get("token").and_then(|v| v.as_str()).unwrap_or("");
                let cookie = data.get("cookie").and_then(|v| v.as_str()).unwrap_or("");
                let workspace = data.get("workspace").and_then(|v| v.as_str()).unwrap_or("unknown");

                if token.is_empty() || !token.starts_with("xox") {
                    println!("  {} Token not found. Make sure you're logged into Slack in the browser.", "✗".red());
                    continue;
                }

                println!("  {} Got token from {}", "✅".green(), workspace.cyan());

                // Save to config
                let savants_home = dirs::home_dir().unwrap_or_default().join(".savants");
                let config_path = savants_home.join("slack.toml");
                let mut lines = vec![
                    format!("user_token = \"{}\"", token),
                ];
                if !cookie.is_empty() {
                    lines.push(format!("cookie = \"{}\"", cookie));
                }
                lines.push(format!("workspace = \"{}\"", workspace));

                // Fetch channel list using the token
                println!("  {} Fetching your channels...", "⏳");
                let client = reqwest::blocking::Client::new();
                let mut req = client.get("https://slack.com/api/conversations.list")
                    .query(&[("types", "public_channel,private_channel"), ("limit", "100")])
                    .header("Authorization", format!("Bearer {}", token));
                if !cookie.is_empty() {
                    req = req.header("Cookie", format!("d={}", cookie));
                }

                let mut channels: Vec<(String, String)> = vec![];
                if let Ok(resp) = req.timeout(std::time::Duration::from_secs(10)).send() {
                    if let Ok(data) = resp.json::<serde_json::Value>() {
                        if let Some(ch_list) = data.get("channels").and_then(|c| c.as_array()) {
                            for ch in ch_list {
                                let id = ch.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                let is_member = ch.get("is_member").and_then(|v| v.as_bool()).unwrap_or(false);
                                if is_member && !id.is_empty() {
                                    channels.push((id.to_string(), name.to_string()));
                                }
                            }
                        }
                    }
                }

                if channels.is_empty() {
                    println!("  Could not fetch channels. You can set the channel manually:");
                    println!("  {} {}", "savants connect slack --user-token".dimmed(), format!("{} --channel C0123ABC", &token[..20]).dimmed());
                } else {
                    println!();
                    println!("  Your channels:");
                    for (i, (_, name)) in channels.iter().enumerate() {
                        println!("    {}. #{}", i + 1, name);
                    }
                    println!();
                    println!("  Which channel for Savants alerts? [1-{}]", channels.len());

                    // Read user input
                    let mut input = String::new();
                    if std::io::stdin().read_line(&mut input).is_ok() {
                        let choice: usize = input.trim().parse().unwrap_or(1);
                        if choice >= 1 && choice <= channels.len() {
                            let (ref ch_id, ref ch_name) = channels[choice - 1];
                            lines.push(format!("channel = \"{}\"", ch_id));
                            println!();
                            println!("  {} Connected to #{} on {}!", "✅".green(), ch_name.cyan(), workspace.cyan());
                        }
                    }
                }

                // Save config
                let config_content = lines.join("\n") + "\n";
                if let Err(e) = std::fs::write(&config_path, &config_content) {
                    eprintln!("{}: {}", "Error".red(), e);
                    return;
                }

                println!();
                println!("  Config saved to {}", config_path.display().to_string().dimmed());
                println!();
                println!("  To start getting alerts in Slack:");
                for line in &lines {
                    let parts: Vec<&str> = line.splitn(2, " = ").collect();
                    if parts.len() == 2 {
                        let key = parts[0].trim();
                        let val = parts[1].trim().trim_matches('"');
                        let env_key = format!("SAVANTS_SLACK_{}", key.to_uppercase());
                        println!("    export {}=\"{}\"", env_key, val);
                    }
                }
                println!("    {} && {}", "savants daemon stop".dimmed(), "savants daemon start".cyan());

                return;
            }
        }
    }
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
