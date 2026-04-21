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

    // Step 1: Request device code from the cloud API
    println!("Requesting device authorization...");
    println!();

    let client = reqwest::Client::new();
    let code_response = match client.post(&format!("{}/auth/device/code", CLOUD_ENDPOINT))
        .send().await
    {
        Ok(resp) if resp.status().is_success() => {
            resp.json::<serde_json::Value>().await.unwrap_or_default()
        }
        Ok(resp) => {
            eprintln!("{}: cloud returned status {}", "Error".red(), resp.status());
            return;
        }
        Err(e) => {
            eprintln!("{}: could not reach savants.cloud: {}", "Error".red(), e);
            eprintln!("  Check your internet connection or try again later.");
            return;
        }
    };

    let device_code = code_response.get("device_code").and_then(|v| v.as_str()).unwrap_or("");
    let user_code = code_response.get("user_code").and_then(|v| v.as_str()).unwrap_or("");
    let default_uri = format!("{}/activate", CLOUD_ENDPOINT);
    let verification_uri = code_response.get("verification_uri").and_then(|v| v.as_str())
        .unwrap_or(&default_uri);
    let interval = code_response.get("interval").and_then(|v| v.as_u64()).unwrap_or(5);

    println!("To authenticate, visit:");
    println!();
    println!("    {}", verification_uri.cyan().bold().underline());
    println!();
    println!("And enter code: {}", user_code.yellow().bold());
    println!();
    println!("Waiting for authentication...");
    println!("{}", "(This will complete automatically once you sign in)".dimmed());

    // Step 2: Poll for token
    for _ in 0..180 {  // 15 minutes max
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;

        let poll_response = match client.post(&format!("{}/auth/device/token", CLOUD_ENDPOINT))
            .json(&serde_json::json!({"device_code": device_code}))
            .send().await
        {
            Ok(resp) => resp,
            Err(_) => continue,
        };

        let status = poll_response.status();
        let body = poll_response.json::<serde_json::Value>().await.unwrap_or_default();

        if status.is_success() {
            // Got the token
            let access_token = body.get("access_token").and_then(|v| v.as_str()).unwrap_or("");
            let org_id = body.get("org_id").and_then(|v| v.as_str()).unwrap_or("");

            // Save to state
            let mut state = State::load();
            state.cloud_device_token = Some(access_token.to_string());
            state.cloud_org = Some(org_id.to_string());
            if let Err(e) = state.save() {
                eprintln!("{}: failed to save state: {}", "Error".red(), e);
                return;
            }

            println!();
            println!("  {} Connected to savants.cloud (org: {})", "●".green(), org_id.cyan());
            println!();
            println!("  Your context engine is now synced to the cloud.");
            println!("  Team members can connect with: {}", "savants connect".cyan());
            return;
        }

        let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
        match error {
            "authorization_pending" => continue,
            "slow_down" => {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
            "expired_token" => {
                eprintln!("{}: device code expired. Run {} again.", "Error".red(), "savants connect".cyan());
                return;
            }
            "access_denied" => {
                eprintln!("{}: authentication denied.", "Error".red());
                return;
            }
            _ => continue,
        }
    }
    eprintln!("{}: authentication timed out. Run {} again.", "Error".red(), "savants connect".cyan());
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
                    println!("  {} {}", "savants connect slack --channel".cyan(), "YOUR_CHANNEL_ID".dimmed());

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

/// Browser-as-proxy Slack setup.
/// The browser makes Slack API calls (with its own HttpOnly cookies) and
/// sends the results to a local server. One paste in the console, fully automatic.
pub async fn slack_from_browser() {
    use tokio::net::TcpListener;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    println!("{}", "Savants Slack Setup".bold());
    println!();
    println!("  {} Starting local server...", "1.".bold());

    let listener = match TcpListener::bind("127.0.0.1:9876").await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}: Cannot bind port 9876: {}", "Error".red(), e);
            return;
        }
    };

    println!("  {} Open your Slack workspace in the browser (e.g. yourcompany.slack.com)", "2.".bold());
    println!("  {} Open DevTools → Console (F12)", "3.".bold());
    println!("  {} Paste these two lines (one at a time):", "4.".bold());
    println!();

    // Step 1: Find the token from network requests in the page
    // Slack embeds the token in script tags or boot data
    println!("     {}", "Step A:".bold());
    println!("     {}", r#"var t="";document.querySelectorAll("script").forEach(s=>{var m=s.textContent.match(/"token":"(xoxc-[^"]+)"/);if(m)t=m[1]});if(!t)t=prompt("Paste token from Network tab (look for xoxc- in any request body):");console.log("Token: "+t.slice(0,30)+"...")"#.dimmed());
    println!();
    println!("     {}", "Step B:".bold());
    println!("     {}", r#"fetch("/api/conversations.list",{method:"POST",credentials:"include",headers:{"Content-Type":"application/x-www-form-urlencoded"},body:"token="+t+"&types=public_channel,private_channel&limit=200"}).then(r=>r.json()).then(d=>{var a=d.channels.filter(c=>c.is_member).map(c=>({id:c.id,name:c.name}));fetch("http://localhost:9876",{method:"POST",mode:"no-cors",body:JSON.stringify({token:t,workspace:location.hostname.split(".")[0],user:"",channels:a})});console.log("Sent "+a.length+" channels to Savants!")})"#.dimmed());
    println!();
    println!("  {} Waiting for data from browser...", "⏳");
    println!();

    // Wait for the browser to POST
    loop {
        let (mut socket, _) = match listener.accept().await {
            Ok(s) => s,
            Err(_) => continue,
        };

        let mut buf = vec![0u8; 65536]; // channels list can be large
        let n = match socket.read(&mut buf).await {
            Ok(n) => n,
            Err(_) => continue,
        };
        let request = String::from_utf8_lossy(&buf[..n]);

        // Send CORS response
        let response = "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Length: 2\r\n\r\nOK";
        let _ = socket.write_all(response.as_bytes()).await;

        if request.starts_with("OPTIONS") { continue; }

        if let Some(body_start) = request.find("\r\n\r\n") {
            let body = &request[body_start + 4..];
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(body) {
                let token = data.get("token").and_then(|v| v.as_str()).unwrap_or("");
                let workspace = data.get("workspace").and_then(|v| v.as_str()).unwrap_or("unknown");
                let user = data.get("user").and_then(|v| v.as_str()).unwrap_or("");

                if token.is_empty() || !token.starts_with("xox") {
                    println!("  {} Token not found in browser data.", "✗".red());
                    continue;
                }

                println!("  {} Authenticated as {} on {}", "✅".green(), user.cyan(), workspace.cyan());

                // Parse channels from browser response
                let channels: Vec<(String, String)> = data.get("channels")
                    .and_then(|c| c.as_array())
                    .map(|arr| arr.iter().filter_map(|ch| {
                        let id = ch.get("id")?.as_str()?.to_string();
                        let name = ch.get("name")?.as_str()?.to_string();
                        Some((id, name))
                    }).collect())
                    .unwrap_or_default();

                if channels.is_empty() {
                    println!("  No channels received.");
                    return;
                }

                println!();
                for (i, (_, name)) in channels.iter().enumerate() {
                    println!("    {}. #{}", (i + 1).to_string().bold(), name);
                }
                println!();
                print!("  Which channel for Savants? [1-{}]: ", channels.len());
                use std::io::Write;
                let _ = std::io::stdout().flush();

                let mut input = String::new();
                if std::io::stdin().read_line(&mut input).is_ok() {
                    let choice: usize = input.trim().parse().unwrap_or(1);
                    if choice >= 1 && choice <= channels.len() {
                        let (ref ch_id, ref ch_name) = channels[choice - 1];

                        // Save config — note: token requires browser cookies to work
                        // The daemon will need to use the browser-proxy pattern too,
                        // OR we store just for graph reads (the browser-proxy handles writes)
                        let savants_home = dirs::home_dir().unwrap_or_default().join(".savants");
                        let _ = std::fs::create_dir_all(&savants_home);
                        let config_path = savants_home.join("slack.toml");
                        let config_content = format!(
                            "user_token = \"{}\"\nchannel = \"{}\"\nworkspace = \"{}\"\nbrowser_proxy = true\n",
                            token, ch_id, workspace
                        );
                        let _ = std::fs::write(&config_path, &config_content);

                        println!();
                        println!("  {} Connected to #{} on {}!", "✅".green(), ch_name.cyan(), workspace.cyan());
                        println!();
                        println!("  {} The token requires browser cookies for API access.", "Note:".yellow());
                        println!("  For the daemon to read Slack, create a Slack app for a standalone token:");
                        println!("    {} → Create App → Bot Token → {}", "api.slack.com/apps".cyan(), "savants connect slack --bot-token xoxb-...".dimmed());
                        println!();
                        println!("  Or use the token directly for context queries (read-only via browser).");
                    }
                }
                return;
            }
        }
    }
}

/// Connect Sentry for error tracking.
pub fn sentry(token: Option<String>, org: Option<String>) {
    println!("{}", "Connecting Sentry...".bold());

    let savants_home = dirs::home_dir().unwrap_or_default().join(".savants");
    let _ = std::fs::create_dir_all(&savants_home);
    let config_path = savants_home.join("sentry.toml");

    if token.is_none() && org.is_none() {
        if config_path.exists() {
            println!("  {} Sentry is configured", "OK".green());
            println!("  Config: {}", config_path.display());
        } else {
            println!("  Sentry is not configured.");
            println!();
            println!("  Run:");
            println!("    {} {}", "savants connect sentry --token".cyan(), "<auth-token> --org <org-slug>".dimmed());
            println!();
            println!("  Get your token at: https://sentry.io/settings/auth-tokens/");
        }
        return;
    }

    let token_val = token.unwrap_or_default();
    let org_val = org.unwrap_or_default();

    // Test connection
    let client = reqwest::blocking::Client::new();
    let resp = client.get(&format!("https://sentry.io/api/0/organizations/{}/", org_val))
        .header("Authorization", format!("Bearer {}", token_val))
        .timeout(std::time::Duration::from_secs(10))
        .send();

    match resp {
        Ok(r) if r.status().is_success() => {
            let config = format!("token = \"{}\"\norg = \"{}\"\n", token_val, org_val);
            let _ = std::fs::write(&config_path, &config);
            println!("  {} Connected to Sentry org: {}", "OK".green(), org_val.cyan());
            println!();
            println!("  Set env vars for the daemon:");
            println!("    export SAVANTS_SENTRY_TOKEN=\"{}\"", token_val);
            println!("    export SAVANTS_SENTRY_ORG=\"{}\"", org_val);
        }
        Ok(r) => {
            eprintln!("  {} Sentry returned HTTP {}", "Error".red(), r.status());
        }
        Err(e) => {
            eprintln!("  {} Failed to reach Sentry: {}", "Error".red(), e);
        }
    }
}

/// Connect Jira for ticket tracking.
pub fn jira(url: Option<String>, user: Option<String>, token: Option<String>, project: Option<String>) {
    println!("{}", "Connecting Jira...".bold());

    let savants_home = dirs::home_dir().unwrap_or_default().join(".savants");
    let _ = std::fs::create_dir_all(&savants_home);
    let config_path = savants_home.join("jira.toml");

    if url.is_none() && token.is_none() {
        if config_path.exists() {
            println!("  {} Jira is configured", "OK".green());
        } else {
            println!("  Jira is not configured.");
            println!();
            println!("  Run:");
            println!("    {} {}", "savants connect jira".cyan(), "--url https://yourcompany.atlassian.net --user you@email.com --token <api-token> --project VSCV".dimmed());
            println!();
            println!("  Get your API token at: https://id.atlassian.net/manage-profile/security/api-tokens");
        }
        return;
    }

    let url_val = url.unwrap_or_default();
    let user_val = user.unwrap_or_default();
    let token_val = token.unwrap_or_default();
    let project_val = project.unwrap_or_else(|| "VSCV".to_string());

    // Test connection
    let client = reqwest::blocking::Client::new();
    let resp = client.get(&format!("{}/rest/api/3/myself", url_val))
        .basic_auth(&user_val, Some(&token_val))
        .timeout(std::time::Duration::from_secs(10))
        .send();

    match resp {
        Ok(r) if r.status().is_success() => {
            let config = format!("url = \"{}\"\nuser = \"{}\"\ntoken = \"{}\"\nproject = \"{}\"\n",
                url_val, user_val, token_val, project_val);
            let _ = std::fs::write(&config_path, &config);
            println!("  {} Connected to Jira: {}", "OK".green(), url_val.cyan());
            println!();
            println!("  Set env vars for the daemon:");
            println!("    export SAVANTS_JIRA_URL=\"{}\"", url_val);
            println!("    export SAVANTS_JIRA_USER=\"{}\"", user_val);
            println!("    export SAVANTS_JIRA_TOKEN=\"{}\"", token_val);
            println!("    export SAVANTS_JIRA_PROJECT=\"{}\"", project_val);
        }
        Ok(r) => {
            eprintln!("  {} Jira returned HTTP {}", "Error".red(), r.status());
        }
        Err(e) => {
            eprintln!("  {} Failed to reach Jira: {}", "Error".red(), e);
        }
    }
}

/// Connect GitHub for PR tracking.
pub fn github(repo: Option<String>) {
    println!("{}", "Connecting GitHub...".bold());

    let savants_home = dirs::home_dir().unwrap_or_default().join(".savants");
    let _ = std::fs::create_dir_all(&savants_home);
    let config_path = savants_home.join("github.toml");

    if repo.is_none() {
        if config_path.exists() {
            println!("  {} GitHub is configured", "OK".green());
        } else {
            println!("  GitHub is not configured.");
            println!();
            println!("  Run:");
            println!("    {} {}", "savants connect github".cyan(), "--repo owner/repo-name".dimmed());
            println!();
            println!("  Make sure gh CLI is authenticated: gh auth login");
        }
        return;
    }

    let repo_val = repo.unwrap_or_default();

    // Test with gh CLI
    let output = std::process::Command::new("gh")
        .args(["repo", "view", &repo_val, "--json", "name"])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let config = format!("repo = \"{}\"\n", repo_val);
            let _ = std::fs::write(&config_path, &config);
            println!("  {} Connected to GitHub: {}", "OK".green(), repo_val.cyan());
            println!();
            println!("  Set env var for the daemon:");
            println!("    export SAVANTS_GITHUB_REPO=\"{}\"", repo_val);
        }
        _ => {
            eprintln!("  {} Failed to access repo. Run: gh auth login", "Error".red());
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
    println!("Your local context are unaffected. Federation sync has stopped.");
}
