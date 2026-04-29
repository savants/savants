export function sentrySetupPage(status?: string, message?: string): string {
  const statusHtml = status === "success"
    ? `<div class="alert success">${message ?? "Sentry integration connected successfully."}</div>`
    : status === "error"
      ? `<div class="alert error">${message ?? "Something went wrong. Please try again."}</div>`
      : "";

  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Savants - Sentry Integration</title>
<style>
*{margin:0;padding:0;box-sizing:border-box}
body{font-family:Inter,-apple-system,sans-serif;background:#0a0a0a;color:#e5e5e5;min-height:100vh;padding:32px 16px}
.container{max-width:640px;margin:0 auto}
.logo{text-align:center;margin-bottom:32px}
.logo span{font-size:1.25rem;font-weight:700;color:#22d3ee;letter-spacing:0.05em}
h1{font-size:1.5rem;font-weight:700;margin-bottom:8px;text-align:center}
.subtitle{text-align:center;color:#737373;margin-bottom:32px;line-height:1.6}
.card{background:#141414;border:1px solid #262626;border-radius:12px;padding:28px;margin-bottom:24px}
.card h2{font-size:1.1rem;font-weight:600;margin-bottom:16px;color:#e5e5e5}
.card h3{font-size:0.95rem;font-weight:600;margin-bottom:8px;color:#a3a3a3}
.step-number{display:inline-flex;align-items:center;justify-content:center;width:28px;height:28px;border-radius:50%;background:#22d3ee;color:#0a0a0a;font-weight:700;font-size:0.85rem;margin-right:10px;flex-shrink:0}
.step-header{display:flex;align-items:center;margin-bottom:12px}
ol.instructions{padding-left:0;list-style:none;margin:0}
ol.instructions li{color:#a3a3a3;line-height:1.8;padding-left:8px;font-size:0.9rem}
ol.instructions li code{background:#1e1e1e;border:1px solid #333;border-radius:4px;padding:2px 6px;font-family:'JetBrains Mono',monospace;font-size:0.8rem;color:#22d3ee}
.form-group{margin-bottom:20px}
.form-group label{display:block;font-size:0.9rem;font-weight:500;margin-bottom:6px;color:#d4d4d4}
.form-group .hint{font-size:0.8rem;color:#737373;margin-top:4px}
input[type="text"],input[type="password"]{width:100%;padding:10px 14px;background:#1e1e1e;border:1px solid #333;border-radius:8px;color:#e5e5e5;font-family:'JetBrains Mono',monospace;font-size:0.85rem;outline:none;transition:border-color 0.2s}
input[type="text"]:focus,input[type="password"]:focus{border-color:#22d3ee}
input[type="text"]::placeholder,input[type="password"]::placeholder{color:#525252}
.checkbox-group{display:flex;align-items:center;gap:10px;margin-bottom:20px}
.checkbox-group input[type="checkbox"]{width:18px;height:18px;accent-color:#22d3ee}
.checkbox-group label{font-size:0.9rem;color:#d4d4d4;margin:0}
.btn{display:inline-block;padding:12px 28px;background:linear-gradient(135deg,#22d3ee,#a78bfa);color:#0a0a0a;font-weight:600;border-radius:10px;border:none;cursor:pointer;font-size:0.95rem;transition:transform 0.2s,opacity 0.2s;width:100%;text-align:center}
.btn:hover{transform:translateY(-1px);opacity:0.95}
.btn:disabled{opacity:0.5;cursor:not-allowed;transform:none}
.btn-secondary{background:#262626;color:#e5e5e5;margin-top:8px}
.btn-secondary:hover{background:#333}
.webhook-url{background:#1e1e1e;border:1px solid #333;border-radius:8px;padding:12px 16px;font-family:'JetBrains Mono',monospace;font-size:0.85rem;color:#22d3ee;word-break:break-all;margin:12px 0}
.alert{padding:14px 18px;border-radius:8px;margin-bottom:24px;font-size:0.9rem;line-height:1.5}
.alert.success{background:#052e16;border:1px solid #166534;color:#4ade80}
.alert.error{background:#2a0a0a;border:1px solid #7f1d1d;color:#f87171}
.divider{border:none;border-top:1px solid #262626;margin:24px 0}
.footer{text-align:center;color:#525252;font-size:0.8rem;margin-top:32px}
.footer a{color:#22d3ee;text-decoration:none}
#result{display:none}
</style>
</head>
<body>
<div class="container">
  <div class="logo"><span>savants</span></div>
  <h1>Sentry Integration</h1>
  <p class="subtitle">Auto-diagnose errors from Sentry using your code graph. Get root cause analysis posted to Slack in seconds.</p>

  ${statusHtml}

  <div class="card">
    <div class="step-header"><span class="step-number">1</span><h2>Create a Sentry Internal Integration</h2></div>
    <ol class="instructions">
      <li>Go to Sentry &rarr; <strong>Settings</strong> &rarr; <strong>Developer Settings</strong> &rarr; <strong>New Internal Integration</strong></li>
      <li>Name: <code>Savants</code></li>
      <li>Webhook URL:</li>
    </ol>
    <div class="webhook-url">https://api.savants.cloud/webhooks/sentry</div>
    <ol class="instructions">
      <li>Permissions:</li>
      <li>&nbsp;&nbsp;Event &amp; Stacktrace: <code>Read</code></li>
      <li>&nbsp;&nbsp;Issue &amp; Event: <code>Read</code></li>
      <li>&nbsp;&nbsp;Project: <code>Read</code></li>
      <li>Webhooks: check <code>issue</code> and <code>event_alert</code></li>
      <li>Click <strong>Save</strong>, then copy the <strong>Token</strong> and <strong>Client Secret</strong></li>
    </ol>
  </div>

  <div class="card">
    <div class="step-header"><span class="step-number">2</span><h2>Connect to Savants</h2></div>
    <form id="sentry-form" method="POST" action="/api/v1/integrations/sentry">
      <div class="form-group">
        <label for="org_slug">Sentry Organization Slug</label>
        <input type="text" id="org_slug" name="org_slug" placeholder="my-org" required autocomplete="off">
        <div class="hint">Found in your Sentry URL: sentry.io/organizations/<strong>my-org</strong>/</div>
      </div>
      <div class="form-group">
        <label for="auth_token">Auth Token</label>
        <input type="password" id="auth_token" name="auth_token" placeholder="sntrys_..." required autocomplete="off">
        <div class="hint">From the Internal Integration you just created</div>
      </div>
      <div class="form-group">
        <label for="client_secret">Client Secret</label>
        <input type="password" id="client_secret" name="client_secret" placeholder="Paste client secret here" required autocomplete="off">
        <div class="hint">Used to verify webhook signatures from Sentry</div>
      </div>
      <div class="form-group">
        <label for="slack_channel">Slack Channel (optional)</label>
        <input type="text" id="slack_channel" name="slack_channel" placeholder="#sentry-alerts" autocomplete="off">
        <div class="hint">Channel where diagnosis results will be posted</div>
      </div>
      <div class="checkbox-group">
        <input type="checkbox" id="auto_diagnose" name="auto_diagnose" checked>
        <label for="auto_diagnose">Auto-diagnose incoming errors</label>
      </div>
      <button type="submit" class="btn" id="submit-btn">Connect Sentry</button>
    </form>
    <div id="result"></div>
  </div>

  <div class="card">
    <div class="step-header"><span class="step-number">3</span><h2>Create an Alert Rule in Sentry</h2></div>
    <ol class="instructions">
      <li>Go to Sentry &rarr; <strong>Alerts</strong> &rarr; <strong>Create Alert</strong></li>
      <li>Choose <strong>Issues</strong> or <strong>Errors</strong> as the alert type</li>
      <li>Set your conditions (e.g., "When a new issue is created")</li>
      <li>Under <strong>Actions</strong>, select <strong>Send a notification via an integration</strong></li>
      <li>Choose the <strong>Savants</strong> integration</li>
      <li>Save the alert rule</li>
    </ol>
    <p style="color:#737373;font-size:0.85rem;margin-top:12px;line-height:1.6">
      Every time this alert fires, Savants will automatically diagnose the error using your code graph and post the root cause analysis to your configured Slack channel.
    </p>
  </div>

  <div class="footer">
    <a href="https://savants.dev">savants.dev</a> &middot; <a href="https://savants.cloud">savants.cloud</a>
  </div>
</div>

<script>
(function() {
  var form = document.getElementById('sentry-form');
  var resultDiv = document.getElementById('result');
  var submitBtn = document.getElementById('submit-btn');

  form.addEventListener('submit', function(e) {
    e.preventDefault();
    submitBtn.disabled = true;
    submitBtn.textContent = 'Connecting...';
    resultDiv.style.display = 'none';

    var payload = {
      org_slug: document.getElementById('org_slug').value,
      auth_token: document.getElementById('auth_token').value,
      client_secret: document.getElementById('client_secret').value,
      slack_channel: document.getElementById('slack_channel').value || undefined,
      auto_diagnose: document.getElementById('auto_diagnose').checked
    };

    fetch('/api/v1/integrations/sentry', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': 'Bearer ' + getToken()
      },
      body: JSON.stringify(payload)
    })
    .then(function(res) { return res.json().then(function(data) { return { ok: res.ok, data: data }; }); })
    .then(function(result) {
      resultDiv.style.display = 'block';
      if (result.ok) {
        resultDiv.className = 'alert success';
        resultDiv.textContent = 'Sentry integration connected successfully for ' + (result.data.sentry_org || payload.org_slug) + '. Webhook URL: ' + result.data.webhook_url;
      } else {
        resultDiv.className = 'alert error';
        resultDiv.textContent = result.data.message || 'Failed to connect. Check your credentials.';
      }
    })
    .catch(function(err) {
      resultDiv.style.display = 'block';
      resultDiv.className = 'alert error';
      resultDiv.textContent = 'Connection error: ' + err.message;
    })
    .finally(function() {
      submitBtn.disabled = false;
      submitBtn.textContent = 'Connect Sentry';
    });
  });

  function getToken() {
    // Try to get token from URL params, cookie, or localStorage
    var params = new URLSearchParams(window.location.search);
    var token = params.get('token');
    if (token) return token;

    // Check localStorage
    try {
      token = localStorage.getItem('savants_token');
      if (token) return token;
    } catch(e) {}

    // Check cookie
    var match = document.cookie.match(/savants_token=([^;]+)/);
    if (match) return match[1];

    return '';
  }
})();
</script>
</body>
</html>`;
}
