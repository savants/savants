// ─── Savants Cloud Dashboard ─────────────────────────────────────────────────
// Server-rendered HTML pages for savants.cloud/dashboard/*
// Each function returns a complete HTML string with inline CSS and JS.

// ─── Shared CSS ──────────────────────────────────────────────────────────────

const SHARED_CSS = `
*{margin:0;padding:0;box-sizing:border-box}
@import url('https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&display=swap');

:root{
  --bg:#0a0a0a;--fg:#e5e5e5;--muted:#737373;--accent:#22d3ee;--accent2:#a78bfa;
  --surface:#141414;--border:#262626;--surface-hover:#1a1a1a;
  --danger:#ef4444;--danger-bg:#2a0a0a;--danger-border:#7f1d1d;
  --success:#4ade80;--success-bg:#052e16;--success-border:#166534;
  --warning:#fbbf24;--warning-bg:#1c1a05;--warning-border:#854d0e;
}

html{font-size:16px;-webkit-font-smoothing:antialiased;-moz-osx-font-smoothing:grayscale}
body{font-family:'Inter',system-ui,-apple-system,sans-serif;background:var(--bg);color:var(--fg);min-height:100vh;overflow-x:hidden}

/* Film grain overlay */
body::before{
  content:'';position:fixed;top:0;left:0;width:100%;height:100%;
  pointer-events:none;z-index:9999;opacity:0.03;
  background-image:url("data:image/svg+xml,%3Csvg viewBox='0 0 256 256' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='noise'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='4' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23noise)'/%3E%3C/svg%3E");
}

/* Layout */
.dash{display:flex;min-height:100vh}
.sidebar{width:240px;background:var(--surface);border-right:1px solid var(--border);display:flex;flex-direction:column;position:fixed;top:0;left:0;bottom:0;z-index:100}
.sidebar-brand{padding:24px 20px;font-size:1.15rem;font-weight:700;color:var(--accent);letter-spacing:0.04em}
.sidebar-brand span{color:var(--muted);font-weight:400}
.sidebar-nav{flex:1;padding:8px 12px;display:flex;flex-direction:column;gap:2px}
.nav-item{display:flex;align-items:center;gap:10px;padding:10px 12px;border-radius:8px;color:var(--muted);text-decoration:none;font-size:0.875rem;font-weight:500;transition:all 0.15s ease}
.nav-item:hover{color:var(--fg);background:rgba(255,255,255,0.04)}
.nav-item.active{color:var(--fg);background:rgba(34,211,238,0.08)}
.nav-item.active::before{content:'';display:inline-block;width:3px;height:16px;border-radius:2px;background:linear-gradient(180deg,var(--accent),var(--accent2));margin-right:-4px}
.nav-icon{width:18px;height:18px;opacity:0.6;flex-shrink:0}
.sidebar-footer{padding:16px 20px;border-top:1px solid var(--border)}
.user-info{font-size:0.8rem;color:var(--muted);margin-bottom:8px;display:flex;align-items:center;gap:8px;overflow:hidden}
.user-info span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:140px}
.user-avatar{width:28px;height:28px;border-radius:50%;background:linear-gradient(135deg,var(--accent),var(--accent2));display:flex;align-items:center;justify-content:center;font-size:0.7rem;font-weight:600;color:var(--bg);flex-shrink:0}
.sidebar-footer a{font-size:0.8rem;color:var(--muted);text-decoration:none;transition:color 0.15s}
.sidebar-footer a:hover{color:var(--fg)}

.content{flex:1;margin-left:240px;min-height:100vh;display:flex;flex-direction:column}
.topbar{padding:20px 32px;border-bottom:1px solid var(--border);display:flex;align-items:center;justify-content:space-between;background:var(--bg)}
.topbar h1{font-size:1.25rem;font-weight:600}
.topbar-actions{display:flex;align-items:center;gap:12px}
.page-content{padding:32px;flex:1}

/* Cards */
.card{background:var(--surface);border:1px solid var(--border);border-radius:16px;padding:24px;transition:border-color 0.2s,box-shadow 0.2s}
.card:hover{border-color:#333;box-shadow:0 0 0 1px rgba(34,211,238,0.05)}
.card-header{display:flex;align-items:center;justify-content:space-between;margin-bottom:16px}
.card-title{font-size:0.95rem;font-weight:600;color:var(--fg)}
.card-subtitle{font-size:0.8rem;color:var(--muted);margin-top:4px}

/* Metric cards */
.metrics-row{display:grid;grid-template-columns:repeat(auto-fit,minmax(200px,1fr));gap:16px;margin-bottom:24px}
.metric-card{background:var(--surface);border:1px solid var(--border);border-radius:16px;padding:20px 24px}
.metric-value{font-family:'JetBrains Mono',monospace;font-size:2rem;font-weight:700;color:var(--fg);line-height:1.1;margin-bottom:4px}
.metric-label{font-size:0.8rem;color:var(--muted);font-weight:500;text-transform:uppercase;letter-spacing:0.05em}
.metric-card.accent .metric-value{background:linear-gradient(135deg,var(--accent),var(--accent2));-webkit-background-clip:text;-webkit-text-fill-color:transparent;background-clip:text}

/* Grid layouts */
.grid-2{display:grid;grid-template-columns:1fr 1fr;gap:24px}
.grid-3{display:grid;grid-template-columns:repeat(3,1fr);gap:16px}

/* Tables */
.table-wrap{overflow-x:auto}
table{width:100%;border-collapse:collapse}
th{text-align:left;font-size:0.75rem;font-weight:600;color:var(--muted);text-transform:uppercase;letter-spacing:0.05em;padding:12px 16px;border-bottom:1px solid var(--border)}
td{padding:14px 16px;border-bottom:1px solid var(--border);font-size:0.875rem;color:var(--fg)}
tr:last-child td{border-bottom:none}
.mono{font-family:'JetBrains Mono',monospace;font-size:0.8rem}

/* Buttons */
.btn{display:inline-flex;align-items:center;gap:8px;padding:10px 20px;border-radius:10px;border:none;cursor:pointer;font-size:0.875rem;font-weight:600;font-family:'Inter',sans-serif;transition:all 0.2s ease;text-decoration:none}
.btn-primary{background:linear-gradient(135deg,var(--accent),var(--accent2));color:var(--bg)}
.btn-primary:hover{transform:translateY(-1px);box-shadow:0 4px 16px rgba(34,211,238,0.25)}
.btn-secondary{background:transparent;border:1px solid var(--border);color:var(--fg)}
.btn-secondary:hover{border-color:#444;background:rgba(255,255,255,0.03)}
.btn-danger{background:transparent;border:1px solid var(--danger-border);color:var(--danger)}
.btn-danger:hover{background:var(--danger-bg)}
.btn-sm{padding:6px 14px;font-size:0.8rem;border-radius:8px}
.btn:disabled{opacity:0.4;cursor:not-allowed;transform:none!important;box-shadow:none!important}

/* Badges */
.badge{display:inline-flex;align-items:center;gap:5px;padding:3px 10px;border-radius:100px;font-size:0.7rem;font-weight:600;text-transform:uppercase;letter-spacing:0.04em}
.badge-green{background:var(--success-bg);border:1px solid var(--success-border);color:var(--success)}
.badge-gray{background:#1a1a1a;border:1px solid var(--border);color:var(--muted)}
.badge-cyan{background:rgba(34,211,238,0.1);border:1px solid rgba(34,211,238,0.2);color:var(--accent)}
.badge-violet{background:rgba(167,139,250,0.1);border:1px solid rgba(167,139,250,0.2);color:var(--accent2)}
.badge-yellow{background:var(--warning-bg);border:1px solid var(--warning-border);color:var(--warning)}
.badge-red{background:var(--danger-bg);border:1px solid var(--danger-border);color:var(--danger)}

/* Status dots */
.status-dot{width:8px;height:8px;border-radius:50%;display:inline-block}
.status-dot.green{background:var(--success);box-shadow:0 0 6px rgba(74,222,128,0.4)}
.status-dot.gray{background:var(--muted)}
.status-dot.yellow{background:var(--warning);box-shadow:0 0 6px rgba(251,191,36,0.4)}

/* Progress bar */
.progress-bar{width:100%;height:6px;background:var(--border);border-radius:3px;overflow:hidden}
.progress-fill{height:100%;background:linear-gradient(90deg,var(--accent),var(--accent2));border-radius:3px;transition:width 0.6s ease}

/* Forms */
.form-group{margin-bottom:20px}
.form-group label{display:block;font-size:0.85rem;font-weight:500;margin-bottom:6px;color:#d4d4d4}
.form-group .hint{font-size:0.75rem;color:var(--muted);margin-top:4px}
input[type="text"],input[type="email"],input[type="password"],select{
  width:100%;padding:10px 14px;background:#1e1e1e;border:1px solid #333;border-radius:8px;
  color:var(--fg);font-family:'Inter',sans-serif;font-size:0.875rem;outline:none;transition:border-color 0.2s
}
input:focus,select:focus{border-color:var(--accent)}
input::placeholder{color:#525252}
select{cursor:pointer;appearance:none;background-image:url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 12 12'%3E%3Cpath d='M3 5l3 3 3-3' stroke='%23737373' stroke-width='1.5' fill='none'/%3E%3C/svg%3E");background-repeat:no-repeat;background-position:right 12px center}
select option{background:var(--surface);color:var(--fg)}

/* Activity feed */
.activity-feed{display:flex;flex-direction:column;gap:0}
.activity-item{display:flex;align-items:center;gap:12px;padding:12px 0;border-bottom:1px solid var(--border)}
.activity-item:last-child{border-bottom:none}
.activity-icon{width:32px;height:32px;border-radius:8px;background:rgba(34,211,238,0.08);display:flex;align-items:center;justify-content:center;flex-shrink:0;font-size:0.75rem;color:var(--accent)}
.activity-text{flex:1;font-size:0.85rem;color:var(--fg)}
.activity-text span{color:var(--muted)}
.activity-time{font-size:0.75rem;color:var(--muted);font-family:'JetBrains Mono',monospace;white-space:nowrap}
.activity-duration{font-size:0.7rem;color:var(--accent);font-family:'JetBrains Mono',monospace}

/* Bar chart (CSS-only) */
.bar-chart{display:flex;align-items:flex-end;gap:6px;height:80px;padding:0}
.bar-col{display:flex;flex-direction:column;align-items:center;gap:4px;flex:1}
.bar{width:100%;border-radius:4px 4px 0 0;background:linear-gradient(180deg,var(--accent),rgba(34,211,238,0.3));transition:height 0.4s ease;min-height:2px}
.bar-label{font-size:0.65rem;color:var(--muted);font-family:'JetBrains Mono',monospace}

/* Integration cards */
.integration-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(280px,1fr));gap:16px}
.integration-card{background:var(--surface);border:1px solid var(--border);border-radius:16px;padding:24px;display:flex;flex-direction:column;gap:16px;transition:border-color 0.2s}
.integration-card:hover{border-color:#333}
.integration-header{display:flex;align-items:center;gap:12px}
.integration-logo{width:40px;height:40px;border-radius:10px;background:#1e1e1e;display:flex;align-items:center;justify-content:center;font-size:1.2rem;font-weight:700;flex-shrink:0}
.integration-info{flex:1}
.integration-name{font-weight:600;font-size:0.95rem}
.integration-desc{font-size:0.8rem;color:var(--muted);margin-top:2px}

/* Alerts */
.alert{padding:14px 18px;border-radius:10px;margin-bottom:20px;font-size:0.85rem;line-height:1.5;display:none}
.alert.visible{display:block}
.alert.success{background:var(--success-bg);border:1px solid var(--success-border);color:var(--success)}
.alert.error{background:var(--danger-bg);border:1px solid var(--danger-border);color:var(--danger)}

/* Skeleton loading */
.skeleton{background:linear-gradient(90deg,#1a1a1a 25%,#222 50%,#1a1a1a 75%);background-size:200% 100%;animation:shimmer 1.5s infinite;border-radius:6px}
@keyframes shimmer{0%{background-position:200% 0}100%{background-position:-200% 0}}
.skeleton-text{height:14px;width:60%;margin-bottom:8px}
.skeleton-number{height:36px;width:80px;margin-bottom:4px}
.skeleton-row{height:48px;width:100%;margin-bottom:4px}

/* Danger zone */
.danger-zone{border:1px solid var(--danger-border);border-radius:16px;padding:24px;margin-top:32px}
.danger-zone h3{color:var(--danger);font-size:0.95rem;margin-bottom:8px}
.danger-zone p{color:var(--muted);font-size:0.85rem;margin-bottom:16px}

/* Modal */
.modal-overlay{position:fixed;top:0;left:0;width:100%;height:100%;background:rgba(0,0,0,0.7);z-index:1000;display:none;align-items:center;justify-content:center;backdrop-filter:blur(4px)}
.modal-overlay.visible{display:flex}
.modal{background:var(--surface);border:1px solid var(--border);border-radius:16px;padding:32px;max-width:480px;width:90%;max-height:90vh;overflow-y:auto}
.modal h2{font-size:1.1rem;font-weight:600;margin-bottom:16px}
.modal-actions{display:flex;gap:12px;justify-content:flex-end;margin-top:24px}

/* Copy button */
.copy-wrap{position:relative;display:flex;align-items:center;gap:8px}
.copy-value{flex:1;padding:10px 14px;background:#1e1e1e;border:1px solid #333;border-radius:8px;font-family:'JetBrains Mono',monospace;font-size:0.8rem;color:var(--accent);word-break:break-all;user-select:all}
.copy-btn{padding:8px 14px;background:#262626;border:1px solid #333;border-radius:8px;color:var(--fg);cursor:pointer;font-size:0.75rem;font-family:'Inter',sans-serif;white-space:nowrap;transition:all 0.2s}
.copy-btn:hover{background:#333}
.copy-btn.copied{background:var(--success-bg);border-color:var(--success-border);color:var(--success)}

/* Plan cards */
.plan-card{border:1px solid var(--border);border-radius:16px;padding:24px;transition:border-color 0.2s}
.plan-card.current{border-color:var(--accent);box-shadow:0 0 0 1px rgba(34,211,238,0.15)}
.plan-name{font-size:1.1rem;font-weight:700;margin-bottom:4px}
.plan-price{font-family:'JetBrains Mono',monospace;font-size:1.5rem;font-weight:700;color:var(--accent);margin-bottom:8px}
.plan-desc{font-size:0.8rem;color:var(--muted);line-height:1.5}

/* Empty state */
.empty-state{text-align:center;padding:48px 24px;color:var(--muted)}
.empty-state .empty-icon{font-size:2.5rem;margin-bottom:16px;opacity:0.3}
.empty-state h3{color:var(--fg);font-size:1rem;margin-bottom:8px}
.empty-state p{font-size:0.85rem;margin-bottom:24px;max-width:360px;margin-left:auto;margin-right:auto;line-height:1.6}

/* Quick actions */
.quick-actions{display:flex;gap:12px;flex-wrap:wrap}

/* Responsive */
@media(max-width:768px){
  .sidebar{transform:translateX(-100%);transition:transform 0.3s ease}
  .sidebar.open{transform:translateX(0)}
  .content{margin-left:0}
  .topbar{padding:16px 20px}
  .topbar h1{font-size:1.1rem}
  .page-content{padding:16px}
  .metrics-row{grid-template-columns:1fr 1fr}
  .grid-2{grid-template-columns:1fr}
  .grid-3{grid-template-columns:1fr}
  .integration-grid{grid-template-columns:1fr}
  .mobile-toggle{display:flex!important}
  /* Tables: stack on mobile */
  table,thead,tbody,th,td,tr{display:block}
  thead{display:none}
  tr{margin-bottom:12px;border:1px solid var(--border);border-radius:10px;padding:12px;background:var(--surface)}
  td{padding:6px 0;border:none;display:flex;justify-content:space-between;align-items:center;gap:8px;font-size:0.85rem}
  td:before{content:attr(data-label);font-weight:600;color:var(--muted);font-size:0.75rem;text-transform:uppercase;letter-spacing:0.05em;flex-shrink:0}
  tr:last-child td{border-bottom:none}
  /* Pricing cards stack */
  .pricing-grid{grid-template-columns:1fr!important}
  .plan-card{padding:20px!important}
  /* Buttons full width */
  .btn{width:100%;justify-content:center}
  .topbar-actions{flex-wrap:wrap;gap:8px}
  /* Modal full screen on mobile */
  .modal{max-width:100%;width:100%;border-radius:12px;padding:24px;margin:16px}
}
.mobile-toggle{display:none;align-items:center;justify-content:center;width:36px;height:36px;border:1px solid var(--border);border-radius:8px;background:transparent;color:var(--fg);cursor:pointer;font-size:1.2rem}
`;

// ─── Shared JS ───────────────────────────────────────────────────────────────

const SHARED_JS = `
(function(){
  // Token management
  var params = new URLSearchParams(window.location.search);
  var tokenParam = params.get('token');
  if (tokenParam) {
    try { localStorage.setItem('savants_token', tokenParam); } catch(e) {}
    // Remove token from URL
    params.delete('token');
    var newUrl = window.location.pathname + (params.toString() ? '?' + params.toString() : '');
    window.history.replaceState({}, '', newUrl);
  }

  window.getToken = function() {
    try { var t = localStorage.getItem('savants_token'); if (t) return t; } catch(e) {}
    var match = document.cookie.match(/savants_token=([^;]+)/);
    if (match) return match[1];
    return '';
  };

  window.apiFetch = function(path, opts) {
    opts = opts || {};
    opts.headers = opts.headers || {};
    opts.headers['Authorization'] = 'Bearer ' + getToken();
    if (opts.body && typeof opts.body === 'object' && !(opts.body instanceof FormData)) {
      opts.headers['Content-Type'] = 'application/json';
      opts.body = JSON.stringify(opts.body);
    }
    return fetch(path, opts).then(function(res) {
      return res.json().then(function(data) { return { ok: res.ok, status: res.status, data: data }; });
    });
  };

  window.showAlert = function(id, type, msg) {
    var el = document.getElementById(id);
    if (!el) return;
    el.className = 'alert visible ' + type;
    el.textContent = msg;
    setTimeout(function() { el.classList.remove('visible'); }, 8000);
  };

  window.timeAgo = function(ts) {
    if (!ts) return 'never';
    var s = Math.floor(Date.now()/1000) - ts;
    if (s < 60) return 'just now';
    if (s < 3600) return Math.floor(s/60) + 'm ago';
    if (s < 86400) return Math.floor(s/3600) + 'h ago';
    if (s < 2592000) return Math.floor(s/86400) + 'd ago';
    return new Date(ts*1000).toLocaleDateString();
  };

  window.formatNumber = function(n) {
    if (n == null) return '0';
    if (n >= 1000000) return (n/1000000).toFixed(1) + 'M';
    if (n >= 1000) return (n/1000).toFixed(1) + 'K';
    return n.toString();
  };

  window.copyToClipboard = function(text, btnEl) {
    navigator.clipboard.writeText(text).then(function() {
      var original = btnEl.textContent;
      btnEl.textContent = 'Copied!';
      btnEl.classList.add('copied');
      setTimeout(function() { btnEl.textContent = original; btnEl.classList.remove('copied'); }, 2000);
    });
  };

  // Mobile sidebar toggle
  var toggle = document.querySelector('.mobile-toggle');
  var sidebar = document.querySelector('.sidebar');
  if (toggle && sidebar) {
    toggle.addEventListener('click', function() { sidebar.classList.toggle('open'); });
  }

  // Check auth
  if (!getToken()) {
    var content = document.querySelector('.page-content');
    if (content) {
      content.innerHTML = '<div class="empty-state"><div class="empty-icon">&#128274;</div><h3>Authentication required</h3><p>Sign in to access your Savants Cloud dashboard.</p><a href="/activate" class="btn btn-primary">Sign in</a></div>';
    }
  }
})();
`;

// ─── SVG Icons ───────────────────────────────────────────────────────────────

const ICONS = {
  overview: '<svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/></svg>',
  activity: '<svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="22 12 18 12 15 21 9 3 6 12 2 12"/></svg>',
  keys: '<svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3m-3.5 3.5L19 4"/></svg>',
  team: '<svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/></svg>',
  integrations: '<svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 2L2 7l10 5 10-5-10-5z"/><path d="M2 17l10 5 10-5"/><path d="M2 12l10 5 10-5"/></svg>',
  billing: '<svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="1" y="4" width="22" height="16" rx="2"/><line x1="1" y1="10" x2="23" y2="10"/></svg>',
  docs: '<svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>',
  settings: '<svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>',
};

// ─── Layout ──────────────────────────────────────────────────────────────────

type NavPage = "overview" | "incidents" | "keys" | "team" | "integrations" | "docs" | "billing" | "settings";

function layout(activePage: NavPage, pageTitle: string, pageContent: string): string {
  const navItems: Array<{ id: NavPage; label: string; href: string; icon: string }> = [
    { id: "overview", label: "Overview", href: "/dashboard", icon: ICONS.overview },
    { id: "incidents", label: "Incidents", href: "/dashboard/incidents", icon: ICONS.overview },
    { id: "keys", label: "API Keys", href: "/dashboard/keys", icon: ICONS.keys },
    { id: "team", label: "Team", href: "/dashboard/team", icon: ICONS.team },
    { id: "integrations", label: "Integrations", href: "/dashboard/integrations", icon: ICONS.integrations },
    { id: "docs", label: "Docs", href: "/dashboard/docs", icon: ICONS.docs },
    { id: "billing", label: "Billing", href: "/dashboard/billing", icon: ICONS.billing },
    { id: "settings", label: "Settings", href: "/dashboard/settings", icon: ICONS.settings },
  ];

  const navHtml = navItems
    .map(
      (item) =>
        `<a href="${item.href}" class="nav-item${item.id === activePage ? " active" : ""}">${item.icon}${item.label}</a>`
    )
    .join("\n      ");

  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${pageTitle} - Savants Cloud</title>
<link rel="icon" href="https://savants.dev/favicon.ico">
<style>${SHARED_CSS}</style>
</head>
<body>
<div class="dash">
  <aside class="sidebar">
    <div class="sidebar-brand">savants<span>.cloud</span></div>
    <nav class="sidebar-nav">
      ${navHtml}
    </nav>
    <div class="sidebar-footer">
      <div class="user-info">
        <div class="user-avatar" id="user-avatar">--</div>
        <span id="user-name">Loading...</span>
      </div>
      <a href="/" id="sign-out-link">Sign out</a>
    </div>
  </aside>
  <main class="content">
    <header class="topbar">
      <div style="display:flex;align-items:center;gap:12px">
        <button class="mobile-toggle" aria-label="Toggle menu">&#9776;</button>
        <h1>${pageTitle}</h1>
      </div>
      <div class="topbar-actions">
        <span id="org-name" style="font-size:0.85rem;color:var(--muted)"></span>
      </div>
    </header>
    <div class="page-content">
      ${pageContent}
    </div>
  </main>
</div>
<script>
${SHARED_JS}
</script>
<script>
// Load user + org info for sidebar
(function(){
  if (!getToken()) return;
  apiFetch('/api/v1/org').then(function(r) {
    if (r.ok) {
      document.getElementById('org-name').textContent = r.data.name || '';
    }
  });
  // Decode JWT for user info
  try {
    var parts = getToken().split('.');
    if (parts.length === 3) {
      var payload = JSON.parse(atob(parts[1].replace(/-/g,'+').replace(/_/g,'/')));
      var email = payload.email || '';
      var name = email.split('@')[0].replace(/[._-]/g, ' ').replace(/\b\w/g, function(c) { return c.toUpperCase(); });
      var initials = name.split(' ').map(function(w) { return w[0]; }).join('').substring(0,2).toUpperCase();
      document.getElementById('user-avatar').textContent = initials;
      document.getElementById('user-name').textContent = name;
      document.getElementById('user-name').title = email;
    }
  } catch(e) {}

  document.getElementById('sign-out-link').addEventListener('click', function(e) {
    e.preventDefault();
    try { localStorage.removeItem('savants_token'); } catch(ex) {}
    document.cookie = 'savants_token=; expires=Thu, 01 Jan 1970 00:00:00 UTC; path=/;';
    window.location.href = '/';
  });
})();
</script>
`;
}

function closeHtml(): string {
  return `</body>\n</html>`;
}

// ─── Overview Page ───────────────────────────────────────────────────────────

export function overviewPage(): string {
  const content = `
      <div id="alert-box" class="alert"></div>

      <!-- Health Score Badge -->
      <div id="health-badge" style="margin-bottom:24px;padding:20px 28px;border-radius:16px;border:1px solid var(--border);background:var(--surface);display:flex;align-items:center;gap:16px">
        <div id="health-dot" style="width:14px;height:14px;border-radius:50%;background:var(--muted);flex-shrink:0"></div>
        <div style="flex:1">
          <div id="health-title" style="font-weight:600;font-size:1rem;color:var(--fg)">Loading...</div>
          <div id="health-subtitle" style="font-size:0.82rem;color:var(--muted);margin-top:2px"></div>
        </div>
      </div>

      <!-- Projects -->
      <div class="card" style="margin-bottom:24px">
        <div class="card-header">
          <div class="card-title">Projects</div>
        </div>
        <div id="projects-list">
          <div class="activity-item"><div class="skeleton" style="width:100%;height:60px;border-radius:8px"></div></div>
        </div>
      </div>

      <div class="grid-2">
        <!-- Left column -->
        <div>
          <!-- Agents -->
          <div class="card" style="margin-bottom:24px">
            <div class="card-header">
              <div class="card-title">Agents</div>
            </div>
            <div id="agents-list">
              <div class="activity-item"><div class="skeleton" style="width:100%;height:40px;border-radius:8px"></div></div>
            </div>
          </div>

          <!-- Integrations -->
          <div class="card" style="margin-bottom:24px">
            <div class="card-header">
              <div class="card-title">Integrations</div>
              <a href="/dashboard/integrations" style="font-size:0.8rem;color:var(--accent);text-decoration:none">Manage</a>
            </div>
            <div id="integration-status">
              <div class="activity-item"><div class="skeleton skeleton-text" style="width:120px"></div></div>
            </div>
          </div>
        </div>

        <!-- Right column -->
        <div>
          <!-- Recent findings -->
          <div class="card" style="margin-bottom:24px">
            <div class="card-header">
              <div class="card-title">Recent Findings</div>
              <label style="font-size:0.75rem;color:var(--muted);display:flex;align-items:center;gap:6px;cursor:pointer"><input type="checkbox" id="show-info" style="accent-color:var(--accent)"> Show info</label>
            </div>
            <div class="activity-feed" id="findings-feed">
              <div class="activity-item"><div class="skeleton" style="width:100%;height:40px;border-radius:8px"></div></div>
            </div>
          </div>

          <!-- Quick actions -->
          <div class="card">
            <div class="card-header">
              <div class="card-title">Quick Actions</div>
            </div>
            <div class="quick-actions">
              <a href="/dashboard/keys" class="btn btn-secondary btn-sm">${ICONS.keys} API Keys</a>
              <a href="/dashboard/team" class="btn btn-secondary btn-sm">${ICONS.team} Team</a>
              <a href="/dashboard/integrations" class="btn btn-secondary btn-sm">${ICONS.integrations} Integrations</a>
              <a href="/dashboard/billing" class="btn btn-secondary btn-sm">${ICONS.billing} Billing</a>
            </div>
          </div>
        </div>
      </div>
  `;

  const js = `
<script>
(function(){
  if (!getToken()) return;

  // Load projects with source counts
  apiFetch('/api/v1/projects').then(function(r) {
    var el = document.getElementById('projects-list');
    if (!r.ok || !r.data.projects || r.data.projects.length === 0) {
      el.innerHTML = '<div class="empty-state" style="padding:24px 0;text-align:center"><p style="font-size:0.85rem;color:var(--muted)">No projects yet.</p><p style="margin-top:8px"><code style="background:var(--surface);padding:4px 8px;border-radius:4px;font-size:0.8rem">savants project create my-app --github org/repo</code></p></div>';
      return;
    }
    el.innerHTML = r.data.projects.map(function(p) {
      var sources = p.source_count || 0;
      var members = p.member_count || 0;
      // Show setup CTA for empty projects
      if (sources === 0 && members === 0) {
        return '<a href="/dashboard/project/' + (p.slug || p.id) + '" class="activity-item" style="padding:12px 0;border-bottom:1px solid var(--border);text-decoration:none;display:flex;cursor:pointer;opacity:0.6">' +
          '<div style="flex:1">' +
            '<div style="font-weight:600;color:var(--fg)">' + p.name + '</div>' +
            '<div style="font-size:0.8rem;color:var(--accent);margin-top:2px">Set up this project &#8594;</div>' +
          '</div>' +
        '</a>';
      }
      return '<a href="/dashboard/project/' + (p.slug || p.id) + '" class="activity-item" style="padding:12px 0;border-bottom:1px solid var(--border);text-decoration:none;display:flex;cursor:pointer">' +
        '<div style="flex:1">' +
          '<div style="font-weight:600;color:var(--fg)">' + p.name + '</div>' +
          '<div style="font-size:0.8rem;color:var(--muted);margin-top:2px">' + sources + ' sources, ' + members + ' members</div>' +
        '</div>' +
        '<div style="font-size:0.75rem;color:var(--muted)">' + (p.slug || '') + ' &#8250;</div>' +
      '</a>';
    }).join('');
  });

  // Load agents (deduplicate by hostname - show only the most recent registration)
  apiFetch('/api/v1/agents').then(function(r) {
    var el = document.getElementById('agents-list');
    if (!r.ok || !r.data.agents || r.data.agents.length === 0) {
      el.innerHTML = '<div class="empty-state" style="padding:16px 0;text-align:center"><p style="font-size:0.85rem;color:var(--muted)">No agents running.</p><p style="margin-top:8px"><code style="background:var(--surface);padding:4px 8px;border-radius:4px;font-size:0.8rem">savants agent start</code></p></div>';
      return;
    }
    // Deduplicate: keep only the latest agent per hostname
    var byName = {};
    r.data.agents.forEach(function(a) {
      var key = a.name || a.hostname || a.id;
      if (!byName[key] || a.last_heartbeat > (byName[key].last_heartbeat || 0)) {
        byName[key] = a;
      }
    });
    var agents = Object.values(byName);
    el.innerHTML = agents.map(function(a) {
      var online = a.online;
      var dotClass = online ? 'green' : 'gray';
      var caps = (a.capabilities || []).length;
      return '<div class="activity-item" style="padding:8px 0">' +
        '<div class="status-dot ' + dotClass + '"></div>' +
        '<div style="flex:1">' +
          '<div class="activity-text">' + (a.name || a.hostname || '?') + '</div>' +
          '<div style="font-size:0.75rem;color:var(--muted)">' + (a.os || '') + ' / ' + (a.arch || '') + ' - ' + caps + ' capabilities</div>' +
        '</div>' +
        '<div style="font-size:0.75rem;color:var(--muted)">' + (online ? 'online' : 'offline') + '</div>' +
      '</div>';
    }).join('');
  });

  // Load integrations
  apiFetch('/api/v1/integrations').then(function(r) {
    var el = document.getElementById('integration-status');
    if (!r.ok) { el.innerHTML = '<div style="color:var(--muted);font-size:0.85rem">Could not load</div>'; return; }

    var types = ['sentry','github','linear','slack','cloudflare','gotify'];
    var connected = {};
    (r.data.integrations || []).forEach(function(i) { connected[i.type] = i.enabled; });

    el.innerHTML = types.map(function(type) {
      var isOn = connected[type];
      var dotClass = isOn ? 'green' : 'gray';
      var label = isOn ? '<span style="color:var(--success)">Connected</span>' : '<a href="/dashboard/integrations" style="font-size:0.75rem;color:var(--accent);text-decoration:none">Set up</a>';
      return '<div class="activity-item" style="padding:4px 0"><div class="status-dot '+dotClass+'"></div><div class="activity-text" style="text-transform:capitalize">'+type+'</div><div style="font-size:0.75rem">'+label+'</div></div>';
    }).join('');
  });

  // Noise suppression: known-good patterns to filter out
  var NOISE_PATTERNS = [
    /efivars.*9[0-9]%/i,                    // EFI vars always ~93%
    /NOT_SPECIFIED\(\d+\)/i,                 // generic TCP cleanup
    /169\.254\.169\.254/i,                   // cloud metadata on non-cloud host
  ];

  function humanizeTitle(title, msg) {
    // Translate cryptic eBPF output to human language
    if (/kernel packet drops.*NOT_SPECIFIED/i.test(msg)) {
      var m = msg.match(/(\d+) packets/);
      return 'Normal TCP cleanup (' + (m ? m[1] : '') + ' connections recycled)';
    }
    return title;
  }

  function smartTimestamp(ts) {
    var sec = Math.round(Date.now()/1000 - ts);
    if (sec < 10) return 'just now';
    if (sec < 60) return sec + 's ago';
    if (sec < 3600) return Math.round(sec/60) + 'm ago';
    if (sec < 86400) return Math.round(sec/3600) + 'h ago';
    return Math.round(sec/86400) + 'd ago';
  }

  // Load recent agent findings
  var allFindings = [];
  apiFetch('/api/v1/agents/events?limit=20').then(function(r) {
    var el = document.getElementById('findings-feed');
    if (!r.ok || !r.data.events || r.data.events.length === 0) {
      el.innerHTML = '<div class="empty-state" style="padding:16px 0;text-align:center"><p style="font-size:0.85rem;color:var(--muted)">No findings yet. Agent will report issues automatically.</p></div>';
      updateHealthBadge([]);
      return;
    }
    allFindings = r.data.events;

    // Suppress noise
    allFindings = allFindings.filter(function(e) {
      var text = (e.title || '') + ' ' + (e.message || '');
      return !NOISE_PATTERNS.some(function(p) { return p.test(text); });
    });

    renderFindings();
    updateHealthBadge(allFindings);
  });

  function renderFindings() {
    var el = document.getElementById('findings-feed');
    var showInfo = document.getElementById('show-info').checked;
    var filtered = allFindings.filter(function(e) {
      return showInfo || e.severity === 'critical' || e.severity === 'warning';
    });
    if (filtered.length === 0) {
      el.innerHTML = '<div style="padding:16px 0;text-align:center;font-size:0.85rem;color:var(--muted)">All clear - no warnings or critical issues.</div>';
      return;
    }
    el.innerHTML = filtered.slice(0, 10).map(function(e) {
      var icon = e.severity === 'critical' ? '&#128308;' : e.severity === 'warning' ? '&#128992;' : '&#128309;';
      var title = humanizeTitle(e.title || '', e.message || '');
      return '<div class="activity-item" style="padding:8px 0">' +
        '<div style="font-size:1.1rem">' + icon + '</div>' +
        '<div style="flex:1">' +
          '<div class="activity-text" style="font-size:0.85rem">' + title.substring(0,70) + '</div>' +
          '<div style="font-size:0.75rem;color:var(--muted)">' + (e.agent || '') + ' - ' + (e.category || '') + '</div>' +
        '</div>' +
        '<div style="font-size:0.75rem;color:var(--muted)">' + smartTimestamp(e.timestamp) + '</div>' +
      '</div>';
    }).join('');
  }

  document.getElementById('show-info').addEventListener('change', renderFindings);

  // Health score badge
  function updateHealthBadge(events) {
    var dot = document.getElementById('health-dot');
    var title = document.getElementById('health-title');
    var sub = document.getElementById('health-subtitle');
    var crits = events.filter(function(e) { return e.severity === 'critical'; }).length;
    var warns = events.filter(function(e) { return e.severity === 'warning'; }).length;
    if (crits > 0) {
      dot.style.background = 'var(--danger)';
      dot.style.boxShadow = '0 0 8px var(--danger)';
      title.textContent = crits + ' critical issue' + (crits > 1 ? 's' : '') + ' detected';
      title.style.color = 'var(--danger)';
      sub.textContent = (warns > 0 ? warns + ' warnings. ' : '') + 'Check findings below for details.';
    } else if (warns > 0) {
      dot.style.background = 'var(--warning)';
      dot.style.boxShadow = '0 0 8px var(--warning)';
      title.textContent = warns + ' warning' + (warns > 1 ? 's' : '');
      title.style.color = 'var(--warning)';
      sub.textContent = 'No critical issues. Review warnings below.';
    } else {
      dot.style.background = 'var(--success)';
      dot.style.boxShadow = '0 0 8px var(--success)';
      title.textContent = 'All systems operational';
      title.style.color = 'var(--success)';
      sub.textContent = 'No issues detected across all agents.';
    }
  }
})();
</script>
`;

  return layout("overview", "Overview", content) + js + closeHtml();
}

// ─── API Keys Page ───────────────────────────────────────────────────────────

export function keysPage(): string {
  const content = `
      <div id="alert-box" class="alert"></div>

      <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:24px">
        <div>
          <div class="card-subtitle">Manage API keys for programmatic access to Savants Cloud.</div>
        </div>
        <button class="btn btn-primary" id="create-key-btn">Create API Key</button>
      </div>

      <div class="card">
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Name</th>
                <th>Key</th>
                <th>Created</th>
                <th>Last Used</th>
                <th></th>
              </tr>
            </thead>
            <tbody id="keys-table">
              <tr><td colspan="5"><div class="skeleton skeleton-row"></div></td></tr>
              <tr><td colspan="5"><div class="skeleton skeleton-row"></div></td></tr>
            </tbody>
          </table>
        </div>
      </div>

      <!-- Create Key Modal -->
      <div class="modal-overlay" id="create-modal">
        <div class="modal">
          <h2>Create API Key</h2>
          <div class="form-group">
            <label for="key-name">Key Name</label>
            <input type="text" id="key-name" placeholder="e.g. production, ci-pipeline, staging">
            <div class="hint">A friendly name to identify this key.</div>
          </div>
          <div class="modal-actions">
            <button class="btn btn-secondary" id="cancel-create">Cancel</button>
            <button class="btn btn-primary" id="confirm-create">Create Key</button>
          </div>
        </div>
      </div>

      <!-- Key Created Modal -->
      <div class="modal-overlay" id="key-created-modal">
        <div class="modal">
          <h2>API Key Created</h2>
          <p style="color:var(--muted);font-size:0.85rem;margin-bottom:16px">Copy this key now. You will not be able to see it again.</p>
          <div class="copy-wrap">
            <div class="copy-value" id="new-key-value"></div>
            <button class="copy-btn" id="copy-key-btn">Copy</button>
          </div>
          <div class="modal-actions" style="margin-top:20px">
            <button class="btn btn-primary" id="close-key-modal">Done</button>
          </div>
        </div>
      </div>

      <!-- Revoke Confirm Modal -->
      <div class="modal-overlay" id="revoke-modal">
        <div class="modal">
          <h2>Revoke API Key</h2>
          <p style="color:var(--muted);font-size:0.85rem;margin-bottom:8px">Are you sure you want to revoke this key? Any applications using it will lose access immediately.</p>
          <p style="color:var(--fg);font-size:0.9rem;font-weight:600" id="revoke-key-name"></p>
          <div class="modal-actions">
            <button class="btn btn-secondary" id="cancel-revoke">Cancel</button>
            <button class="btn btn-danger" id="confirm-revoke">Revoke Key</button>
          </div>
        </div>
      </div>
  `;

  const js = `
<script>
(function(){
  if (!getToken()) return;

  var revokeKeyId = null;

  function loadKeys() {
    apiFetch('/api/v1/org/keys').then(function(r) {
      var tbody = document.getElementById('keys-table');
      if (!r.ok) {
        tbody.innerHTML = '<tr><td colspan="5" style="color:var(--danger)">Failed to load keys: ' + (r.data.message || 'Unknown error') + '</td></tr>';
        return;
      }
      var keys = r.data.keys || [];
      if (keys.length === 0) {
        tbody.innerHTML = '<tr><td colspan="5"><div class="empty-state" style="padding:32px 0"><h3>No API keys yet</h3><p>Create your first API key to start making authenticated requests.</p></div></td></tr>';
        return;
      }
      tbody.innerHTML = keys.map(function(k) {
        return '<tr>' +
          '<td style="font-weight:500">' + (k.name || 'Unnamed') + '</td>' +
          '<td><span class="mono">' + k.prefix + '****</span></td>' +
          '<td style="color:var(--muted)">' + new Date(k.created_at * 1000).toLocaleDateString() + '</td>' +
          '<td style="color:var(--muted)">' + timeAgo(k.last_used_at) + '</td>' +
          '<td style="text-align:right"><button class="btn btn-danger btn-sm revoke-btn" data-id="' + k.id + '" data-name="' + (k.name || k.prefix) + '">Revoke</button></td>' +
          '</tr>';
      }).join('');

      // Attach revoke listeners
      document.querySelectorAll('.revoke-btn').forEach(function(btn) {
        btn.addEventListener('click', function() {
          revokeKeyId = btn.dataset.id;
          document.getElementById('revoke-key-name').textContent = btn.dataset.name;
          document.getElementById('revoke-modal').classList.add('visible');
        });
      });
    });
  }

  loadKeys();

  // Create key flow
  document.getElementById('create-key-btn').addEventListener('click', function() {
    document.getElementById('key-name').value = '';
    document.getElementById('create-modal').classList.add('visible');
  });

  document.getElementById('cancel-create').addEventListener('click', function() {
    document.getElementById('create-modal').classList.remove('visible');
  });

  document.getElementById('confirm-create').addEventListener('click', function() {
    var name = document.getElementById('key-name').value.trim();
    if (!name) { showAlert('alert-box', 'error', 'Key name is required'); return; }

    var btn = document.getElementById('confirm-create');
    btn.disabled = true;
    btn.textContent = 'Creating...';

    apiFetch('/api/v1/org/keys', { method: 'POST', body: { name: name } }).then(function(r) {
      document.getElementById('create-modal').classList.remove('visible');
      btn.disabled = false;
      btn.textContent = 'Create Key';

      if (r.ok) {
        document.getElementById('new-key-value').textContent = r.data.key;
        document.getElementById('key-created-modal').classList.add('visible');
        loadKeys();
      } else {
        showAlert('alert-box', 'error', r.data.message || 'Failed to create key');
      }
    }).catch(function() {
      btn.disabled = false;
      btn.textContent = 'Create Key';
      showAlert('alert-box', 'error', 'Network error');
    });
  });

  document.getElementById('copy-key-btn').addEventListener('click', function() {
    copyToClipboard(document.getElementById('new-key-value').textContent, this);
  });

  document.getElementById('close-key-modal').addEventListener('click', function() {
    document.getElementById('key-created-modal').classList.remove('visible');
  });

  // Revoke flow
  document.getElementById('cancel-revoke').addEventListener('click', function() {
    document.getElementById('revoke-modal').classList.remove('visible');
    revokeKeyId = null;
  });

  document.getElementById('confirm-revoke').addEventListener('click', function() {
    if (!revokeKeyId) return;
    var btn = document.getElementById('confirm-revoke');
    btn.disabled = true;
    btn.textContent = 'Revoking...';

    apiFetch('/api/v1/org/keys/' + revokeKeyId, { method: 'DELETE' }).then(function(r) {
      document.getElementById('revoke-modal').classList.remove('visible');
      btn.disabled = false;
      btn.textContent = 'Revoke Key';
      revokeKeyId = null;

      if (r.ok) {
        showAlert('alert-box', 'success', 'API key revoked successfully');
        loadKeys();
      } else {
        showAlert('alert-box', 'error', r.data.message || 'Failed to revoke key');
      }
    }).catch(function() {
      btn.disabled = false;
      btn.textContent = 'Revoke Key';
      showAlert('alert-box', 'error', 'Network error');
    });
  });

  // Close modals on overlay click
  document.querySelectorAll('.modal-overlay').forEach(function(overlay) {
    overlay.addEventListener('click', function(e) {
      if (e.target === overlay) overlay.classList.remove('visible');
    });
  });
})();
</script>
`;

  return layout("keys", "API Keys", content) + js + closeHtml();
}

// ─── Team Page ───────────────────────────────────────────────────────────────

export function teamPage(): string {
  const content = `
      <div id="alert-box" class="alert"></div>

      <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:24px">
        <div>
          <div class="card-subtitle">Manage your organization's team members and roles.</div>
        </div>
        <button class="btn btn-primary" id="invite-btn">Invite Member</button>
      </div>

      <div class="card">
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Member</th>
                <th>Email</th>
                <th>Role</th>
                <th>Joined</th>
              </tr>
            </thead>
            <tbody id="members-table">
              <tr><td colspan="4"><div class="skeleton skeleton-row"></div></td></tr>
              <tr><td colspan="4"><div class="skeleton skeleton-row"></div></td></tr>
            </tbody>
          </table>
        </div>
      </div>

      <!-- Invite Modal -->
      <div class="modal-overlay" id="invite-modal">
        <div class="modal">
          <h2>Invite Team Member</h2>
          <div class="form-group">
            <label for="invite-email">Email Address</label>
            <input type="email" id="invite-email" placeholder="colleague@company.com">
          </div>
          <div class="form-group">
            <label for="invite-role">Role</label>
            <select id="invite-role">
              <option value="member">Member</option>
              <option value="admin">Admin</option>
            </select>
            <div class="hint">Admins can manage API keys, integrations, and billing.</div>
          </div>
          <div class="modal-actions">
            <button class="btn btn-secondary" id="cancel-invite">Cancel</button>
            <button class="btn btn-primary" id="confirm-invite">Send Invite</button>
          </div>
        </div>
      </div>
  `;

  const js = `
<script>
(function(){
  if (!getToken()) return;

  function loadMembers() {
    apiFetch('/api/v1/org/members').then(function(r) {
      var tbody = document.getElementById('members-table');
      if (!r.ok) {
        tbody.innerHTML = '<tr><td colspan="4" style="color:var(--danger)">Failed to load members</td></tr>';
        return;
      }
      var members = r.data.members || [];
      if (members.length === 0) {
        tbody.innerHTML = '<tr><td colspan="4"><div class="empty-state" style="padding:32px 0"><h3>No team members</h3><p>Invite your first teammate to collaborate.</p></div></td></tr>';
        return;
      }
      tbody.innerHTML = members.map(function(m) {
        var initials = (m.name || m.email || '??').substring(0,2).toUpperCase();
        var roleBadge = m.role === 'owner' ? '<span class="badge badge-cyan">Owner</span>'
          : m.role === 'admin' ? '<span class="badge badge-violet">Admin</span>'
          : '<span class="badge badge-gray">Member</span>';
        return '<tr>' +
          '<td data-label="Member"><div style="display:flex;align-items:center;gap:10px"><div class="user-avatar">' + initials + '</div><span style="font-weight:500">' + (m.name || 'Unnamed') + '</span></div></td>' +
          '<td data-label="Email" class="mono" style="color:var(--muted)">' + (m.email || '-') + '</td>' +
          '<td data-label="Role">' + roleBadge + '</td>' +
          '<td data-label="Joined" style="color:var(--muted)">' + new Date(m.created_at * 1000).toLocaleDateString() + '</td>' +
          '</tr>';
      }).join('');
    });
  }

  loadMembers();

  // Invite flow
  document.getElementById('invite-btn').addEventListener('click', function() {
    document.getElementById('invite-email').value = '';
    document.getElementById('invite-role').value = 'member';
    document.getElementById('invite-modal').classList.add('visible');
  });

  document.getElementById('cancel-invite').addEventListener('click', function() {
    document.getElementById('invite-modal').classList.remove('visible');
  });

  document.getElementById('confirm-invite').addEventListener('click', function() {
    var email = document.getElementById('invite-email').value.trim();
    var role = document.getElementById('invite-role').value;

    if (!email) { showAlert('alert-box', 'error', 'Email is required'); return; }

    var btn = document.getElementById('confirm-invite');
    btn.disabled = true;
    btn.textContent = 'Sending...';

    apiFetch('/api/v1/org/members/invite', { method: 'POST', body: { email: email, role: role } }).then(function(r) {
      document.getElementById('invite-modal').classList.remove('visible');
      btn.disabled = false;
      btn.textContent = 'Send Invite';

      if (r.ok) {
        showAlert('alert-box', 'success', 'Invitation sent to ' + email);
        loadMembers();
      } else {
        showAlert('alert-box', 'error', r.data.message || 'Failed to send invite');
      }
    }).catch(function() {
      btn.disabled = false;
      btn.textContent = 'Send Invite';
      showAlert('alert-box', 'error', 'Network error');
    });
  });

  // Close modals on overlay click
  document.querySelectorAll('.modal-overlay').forEach(function(overlay) {
    overlay.addEventListener('click', function(e) {
      if (e.target === overlay) overlay.classList.remove('visible');
    });
  });
})();
</script>
`;

  return layout("team", "Team", content) + js + closeHtml();
}

// ─── Integrations Page ───────────────────────────────────────────────────────

export function integrationsPage(): string {
  const content = `
      <div id="alert-box" class="alert"></div>

      <div style="margin-bottom:24px">
        <div class="card-subtitle">Connect Savants to your existing tools for automated error diagnosis and code intelligence.</div>
      </div>

      <div class="integration-grid" id="integrations-grid">
        <!-- Sentry -->
        <div class="integration-card" id="card-sentry">
          <div class="integration-header">
            <div class="integration-logo" style="background:#362d59;color:#fff;font-size:0.9rem;font-weight:700">S</div>
            <div class="integration-info">
              <div class="integration-name">Sentry</div>
              <div class="integration-desc">Auto-diagnose production errors</div>
            </div>
            <span class="badge badge-gray" id="sentry-badge">Checking...</span>
          </div>
          <p style="font-size:0.8rem;color:var(--muted);line-height:1.5">When Sentry catches an error, Savants automatically diagnoses the root cause using your code graph and posts the analysis to Slack.</p>
          <div id="sentry-action">
            <a href="/integrations/sentry" class="btn btn-secondary btn-sm" style="width:100%;justify-content:center">Connect Sentry</a>
          </div>
        </div>

        <!-- GitHub -->
        <div class="integration-card" id="card-github">
          <div class="integration-header">
            <div class="integration-logo" style="background:#161b22;color:#fff;font-size:1.1rem">&#9733;</div>
            <div class="integration-info">
              <div class="integration-name">GitHub</div>
              <div class="integration-desc">PR risk analysis and code review</div>
            </div>
            <span class="badge badge-gray" id="github-badge">Checking...</span>
          </div>
          <p style="font-size:0.8rem;color:var(--muted);line-height:1.5">Analyze pull requests for blast radius, suggest reviewers, and surface risky changes before they ship.</p>
          <div id="github-action">
            <a href="/auth/github?redirect=https://savants.cloud/integrations/github" class="btn btn-secondary btn-sm" style="width:100%;justify-content:center;text-decoration:none">Connect GitHub</a>
          </div>
        </div>

        <!-- Slack -->
        <div class="integration-card" id="card-slack">
          <div class="integration-header">
            <div class="integration-logo" style="background:#4a154b;color:#fff;font-size:0.9rem;font-weight:700">#</div>
            <div class="integration-info">
              <div class="integration-name">Slack</div>
              <div class="integration-desc">Diagnosis alerts and notifications</div>
            </div>
            <span class="badge badge-gray" id="slack-badge">Checking...</span>
          </div>
          <p style="font-size:0.8rem;color:var(--muted);line-height:1.5">Receive root cause analyses and architecture insights directly in your Slack channels.</p>
          <div id="slack-action">
            <button class="btn btn-secondary btn-sm" style="width:100%;justify-content:center" onclick="alert('Run savants connect slack from the CLI to connect Slack.')">Connect Slack</button>
          </div>
        </div>

        <!-- PagerDuty -->
        <div class="integration-card" style="opacity:0.6">
          <div class="integration-header">
            <div class="integration-logo" style="background:#06ac38;color:#fff;font-size:0.8rem;font-weight:700">PD</div>
            <div class="integration-info">
              <div class="integration-name">PagerDuty</div>
              <div class="integration-desc">Incident enrichment</div>
            </div>
            <span class="badge badge-yellow">Coming Soon</span>
          </div>
          <p style="font-size:0.8rem;color:var(--muted);line-height:1.5">Automatically enrich PagerDuty incidents with code context and root cause analysis.</p>
        </div>

        <!-- Jira -->
        <div class="integration-card" style="opacity:0.6">
          <div class="integration-header">
            <div class="integration-logo" style="background:#0052cc;color:#fff;font-size:0.8rem;font-weight:700">J</div>
            <div class="integration-info">
              <div class="integration-name">Jira</div>
              <div class="integration-desc">Auto-create tickets from diagnoses</div>
            </div>
            <span class="badge badge-yellow">Coming Soon</span>
          </div>
          <p style="font-size:0.8rem;color:var(--muted);line-height:1.5">Turn diagnosis results into well-documented Jira tickets with all the context engineers need.</p>
        </div>

        <!-- Datadog -->
        <div class="integration-card" style="opacity:0.6">
          <div class="integration-header">
            <div class="integration-logo" style="background:#632ca6;color:#fff;font-size:0.8rem;font-weight:700">DD</div>
            <div class="integration-info">
              <div class="integration-name">Datadog</div>
              <div class="integration-desc">Trace-to-code correlation</div>
            </div>
            <span class="badge badge-yellow">Coming Soon</span>
          </div>
          <p style="font-size:0.8rem;color:var(--muted);line-height:1.5">Correlate Datadog traces and APM data with your code graph for faster debugging.</p>
        </div>
      </div>
  `;

  const js = `
<script>
(function(){
  if (!getToken()) return;

  apiFetch('/api/v1/integrations').then(function(r) {
    if (!r.ok) return;

    var connected = {};
    (r.data.integrations || []).forEach(function(i) { connected[i.type] = i; });

    // Sentry
    if (connected.sentry && connected.sentry.enabled) {
      document.getElementById('sentry-badge').className = 'badge badge-green';
      document.getElementById('sentry-badge').textContent = 'Connected';
      document.getElementById('sentry-action').innerHTML = '<a href="/integrations/sentry" class="btn btn-secondary btn-sm" style="width:100%;justify-content:center">Manage</a>';
    } else {
      document.getElementById('sentry-badge').className = 'badge badge-gray';
      document.getElementById('sentry-badge').textContent = 'Not connected';
    }

    // GitHub
    if (connected.github && connected.github.enabled) {
      document.getElementById('github-badge').className = 'badge badge-green';
      document.getElementById('github-badge').textContent = 'Connected';
      document.getElementById('github-action').innerHTML = '<button class="btn btn-secondary btn-sm" style="width:100%;justify-content:center">Manage</button>';
    } else {
      document.getElementById('github-badge').className = 'badge badge-gray';
      document.getElementById('github-badge').textContent = 'Not connected';
    }

    // Slack
    if (connected.slack && connected.slack.enabled) {
      document.getElementById('slack-badge').className = 'badge badge-green';
      document.getElementById('slack-badge').textContent = 'Connected';
      document.getElementById('slack-action').innerHTML = '<button class="btn btn-secondary btn-sm" style="width:100%;justify-content:center">Manage</button>';
    } else {
      document.getElementById('slack-badge').className = 'badge badge-gray';
      document.getElementById('slack-badge').textContent = 'Not connected';
    }
  }).catch(function() {
    ['sentry','github','slack'].forEach(function(t) {
      var badge = document.getElementById(t + '-badge');
      if (badge) { badge.className = 'badge badge-gray'; badge.textContent = 'Unknown'; }
    });
  });
})();
</script>
`;

  return layout("integrations", "Integrations", content) + js + closeHtml();
}

// ─── Billing Page ────────────────────────────────────────────────────────────

export function billingPage(): string {
  const content = `
      <div id="alert-box" class="alert"></div>

      <!-- Current plan -->
      <div class="card" style="margin-bottom:24px">
        <div class="card-header">
          <div class="card-title">Current Plan</div>
        </div>
        <div style="display:flex;align-items:center;gap:16px;margin-bottom:16px">
          <div>
            <span class="badge badge-cyan" id="plan-badge" style="font-size:0.85rem;padding:6px 16px">
              <span class="skeleton" style="display:inline-block;width:40px;height:12px"></span>
            </span>
          </div>
          <div id="plan-details" style="flex:1;color:var(--muted);font-size:0.85rem"></div>
        </div>
        <div id="subscription-info"></div>
      </div>

      <!-- Usage this month -->
      <div class="grid-2" style="margin-bottom:24px">
        <div class="card">
          <div class="card-header">
            <div class="card-title">This Month's Usage</div>
          </div>
          <div class="metrics-row" style="margin-bottom:0">
            <div>
              <div class="metric-value mono" id="billing-total-calls" style="font-size:1.5rem">
                <span class="skeleton skeleton-number" style="height:24px">&nbsp;</span>
              </div>
              <div class="metric-label">Total Calls</div>
            </div>
            <div>
              <div class="metric-value mono" id="billing-tokens" style="font-size:1.5rem">
                <span class="skeleton skeleton-number" style="height:24px">&nbsp;</span>
              </div>
              <div class="metric-label">Tokens Used</div>
            </div>
          </div>
        </div>

        <div class="card">
          <div class="card-header">
            <div class="card-title">Usage by Tool</div>
          </div>
          <div class="bar-chart" id="billing-chart" style="height:100px">
            <div style="text-align:center;width:100%;padding:20px 0;color:var(--muted);font-size:0.85rem">Loading...</div>
          </div>
        </div>
      </div>

      <!-- Tool breakdown table -->
      <div class="card" style="margin-bottom:24px">
        <div class="card-header">
          <div class="card-title">Breakdown by Tool</div>
        </div>
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Tool</th>
                <th>Calls</th>
                <th>Tokens In</th>
                <th>Tokens Out</th>
                <th>Avg Duration</th>
              </tr>
            </thead>
            <tbody id="billing-breakdown">
              <tr><td colspan="5"><div class="skeleton skeleton-row"></div></td></tr>
            </tbody>
          </table>
        </div>
      </div>

      <!-- Pricing tiers -->
      <div class="card">
        <div class="card-header">
          <div class="card-title">Plans</div>
        </div>
        <div class="grid-3" id="plan-cards">
          <div class="plan-card" id="plan-local">
            <div class="plan-name">Local</div>
            <div class="plan-price">Free<span style="font-size:0.7rem;color:var(--muted)"> forever</span></div>
            <div class="plan-desc">Unlimited local queries. semantic_search, file_skeleton, callers, where_used. No account needed.</div>
          </div>
          <div class="plan-card" id="plan-cloud">
            <div class="plan-name">Cloud</div>
            <div class="plan-price">Pay per call</div>
            <div class="plan-desc">10 free/month, then per call. diagnose_error $5, pr_risk $2, diff_impact $1, radar $1. No minimums.</div>
            <button class="btn btn-primary btn-sm" style="margin-top:16px;width:100%;justify-content:center" id="upgrade-btn">Add Payment Method</button>
          </div>
          <div class="plan-card" id="plan-enterprise">
            <div class="plan-name">Enterprise</div>
            <div class="plan-price">Volume discounts</div>
            <div class="plan-desc">SSO, audit logs, SLA, dedicated support. Contact sales for volume pricing.</div>
            <a href="mailto:hello@miguel.engineer" class="btn btn-secondary btn-sm" style="margin-top:16px;width:100%;justify-content:center;text-decoration:none">Contact Sales</a>
          </div>
        </div>
      </div>
  `;

  const js = `
<script>
(function(){
  if (!getToken()) return;

  // Load billing info
  apiFetch('/api/v1/billing').then(function(r) {
    if (!r.ok) return;
    var d = r.data;

    var planEl = document.getElementById('plan-badge');
    var plan = (d.plan || 'free').charAt(0).toUpperCase() + (d.plan || 'free').slice(1);
    planEl.innerHTML = plan;

    // Highlight current plan
    var currentPlanId = 'plan-' + (d.plan || 'free');
    var el = document.getElementById(currentPlanId);
    if (el) el.classList.add('current');

    // Hide upgrade button if already on cloud/enterprise
    if (d.plan === 'cloud' || d.plan === 'enterprise') {
      var upgradeBtn = document.getElementById('upgrade-btn');
      if (upgradeBtn) { upgradeBtn.textContent = 'Current Plan'; upgradeBtn.disabled = true; }
    }

    // Subscription info
    if (d.subscription) {
      var sub = d.subscription;
      var periodEnd = new Date(sub.current_period_end * 1000).toLocaleDateString();
      var statusBadge = sub.status === 'active' ? '<span class="badge badge-green">Active</span>'
        : sub.status === 'past_due' ? '<span class="badge badge-red">Past Due</span>'
        : '<span class="badge badge-gray">' + sub.status + '</span>';
      document.getElementById('subscription-info').innerHTML =
        '<div style="display:flex;align-items:center;gap:12px;font-size:0.85rem;color:var(--muted)">' +
        statusBadge + ' Next billing: ' + periodEnd +
        (sub.cancel_at_period_end ? ' <span class="badge badge-yellow">Canceling</span>' : '') +
        '</div>';
    }

    document.getElementById('plan-details').textContent = d.pricing ? d.pricing[d.plan || 'free']?.description || '' : '';
  });

  // Load usage
  apiFetch('/api/v1/usage').then(function(r) {
    if (!r.ok) return;
    var d = r.data;

    document.getElementById('billing-total-calls').textContent = d.total_calls;
    var totalTokens = (d.breakdown || []).reduce(function(s,b){return s + (b.tokens_in||0) + (b.tokens_out||0)}, 0);
    document.getElementById('billing-tokens').textContent = formatNumber(totalTokens);

    // Chart
    var chartEl = document.getElementById('billing-chart');
    if (d.breakdown && d.breakdown.length > 0) {
      var maxCalls = Math.max.apply(null, d.breakdown.map(function(b){return b.calls}));
      chartEl.innerHTML = d.breakdown.map(function(b) {
        var h = maxCalls > 0 ? Math.max(4, (b.calls / maxCalls) * 90) : 4;
        var shortName = b.tool.replace('savants_','').substring(0,10);
        return '<div class="bar-col"><div class="bar" style="height:'+h+'px" title="'+b.tool+': '+b.calls+'"></div><div class="bar-label">'+shortName+'</div></div>';
      }).join('');
    } else {
      chartEl.innerHTML = '<div style="text-align:center;width:100%;padding:24px 0;color:var(--muted);font-size:0.85rem">No usage data yet</div>';
    }

    // Breakdown table
    var tbody = document.getElementById('billing-breakdown');
    if (d.breakdown && d.breakdown.length > 0) {
      tbody.innerHTML = d.breakdown.map(function(b) {
        var avgMs = b.calls > 0 ? Math.round(b.duration_ms / b.calls) : 0;
        return '<tr>' +
          '<td class="mono">' + b.tool + '</td>' +
          '<td class="mono">' + b.calls + '</td>' +
          '<td class="mono">' + formatNumber(b.tokens_in) + '</td>' +
          '<td class="mono">' + formatNumber(b.tokens_out) + '</td>' +
          '<td class="mono">' + avgMs + 'ms</td>' +
          '</tr>';
      }).join('');
    } else {
      tbody.innerHTML = '<tr><td colspan="5" style="text-align:center;color:var(--muted);padding:24px">No usage data this month</td></tr>';
    }
  });

  // Upgrade button
  var upgradeBtn = document.getElementById('upgrade-btn');
  if (upgradeBtn) {
    upgradeBtn.addEventListener('click', function() {
      upgradeBtn.disabled = true;
      upgradeBtn.textContent = 'Redirecting...';

      apiFetch('/api/v1/billing/checkout', {
        method: 'POST',
        body: {
          success_url: window.location.origin + '/dashboard/billing?checkout=success',
          cancel_url: window.location.origin + '/dashboard/billing?checkout=cancelled'
        }
      }).then(function(r) {
        if (r.ok && r.data.checkout_url) {
          window.location.href = r.data.checkout_url;
        } else {
          upgradeBtn.disabled = false;
          upgradeBtn.textContent = 'Upgrade';
          showAlert('alert-box', 'error', r.data.message || 'Failed to start checkout');
        }
      }).catch(function() {
        upgradeBtn.disabled = false;
        upgradeBtn.textContent = 'Upgrade';
        showAlert('alert-box', 'error', 'Network error');
      });
    });
  }

  // Check for checkout result
  var params = new URLSearchParams(window.location.search);
  if (params.get('checkout') === 'success') {
    showAlert('alert-box', 'success', 'Payment successful! Your plan has been upgraded.');
  } else if (params.get('checkout') === 'cancelled') {
    showAlert('alert-box', 'error', 'Checkout was cancelled.');
  }
})();
</script>
`;

  return layout("billing", "Billing", content) + js + closeHtml();
}

// ─── Settings Page ───────────────────────────────────────────────────────────

export function settingsPage(): string {
  const content = `
      <div id="alert-box" class="alert"></div>

      <!-- Org settings -->
      <div class="card" style="margin-bottom:24px">
        <div class="card-header">
          <div class="card-title">Organization</div>
        </div>
        <div class="form-group">
          <label for="org-name-input">Organization Name</label>
          <input type="text" id="org-name-input" placeholder="Loading...">
        </div>
        <div class="form-group">
          <label>Organization Slug</label>
          <input type="text" id="org-slug-input" disabled style="opacity:0.6;cursor:not-allowed">
          <div class="hint">The slug is used in API URLs and cannot be changed.</div>
        </div>
        <div class="form-group">
          <label>Plan</label>
          <div style="display:flex;align-items:center;gap:12px">
            <input type="text" id="org-plan-input" disabled style="opacity:0.6;cursor:not-allowed;width:auto">
            <a href="/dashboard/billing" style="font-size:0.85rem;color:var(--accent);text-decoration:none">Change plan</a>
          </div>
        </div>
        <div class="form-group">
          <label>Created</label>
          <input type="text" id="org-created-input" disabled style="opacity:0.6;cursor:not-allowed">
        </div>
        <button class="btn btn-primary" id="save-settings-btn" disabled>Save Changes</button>
      </div>

      <!-- Danger zone -->
      <div class="danger-zone">
        <h3>Danger Zone</h3>
        <p>Deleting your organization is permanent. All API keys, integrations, usage data, and team memberships will be permanently removed. This action cannot be undone.</p>
        <button class="btn btn-danger" id="delete-org-btn">Delete Organization</button>
      </div>

      <!-- Delete confirmation modal -->
      <div class="modal-overlay" id="delete-modal">
        <div class="modal">
          <h2 style="color:var(--danger)">Delete Organization</h2>
          <p style="color:var(--muted);font-size:0.85rem;margin-bottom:16px">This will permanently delete your organization, all API keys, integrations, and team memberships. This action cannot be undone.</p>
          <div class="form-group">
            <label>Type the organization slug to confirm:</label>
            <input type="text" id="confirm-slug" placeholder="org-slug">
          </div>
          <div class="modal-actions">
            <button class="btn btn-secondary" id="cancel-delete">Cancel</button>
            <button class="btn btn-danger" id="confirm-delete" disabled>Delete Forever</button>
          </div>
        </div>
      </div>
  `;

  const js = `
<script>
(function(){
  if (!getToken()) return;

  var orgSlug = '';

  apiFetch('/api/v1/org').then(function(r) {
    if (!r.ok) return;
    var d = r.data;

    document.getElementById('org-name-input').value = d.name || '';
    document.getElementById('org-slug-input').value = d.slug || '';
    document.getElementById('org-plan-input').value = (d.plan || 'free').charAt(0).toUpperCase() + (d.plan || 'free').slice(1);
    document.getElementById('org-created-input').value = new Date(d.created_at * 1000).toLocaleDateString();

    orgSlug = d.slug || '';

    // Enable save on change
    document.getElementById('org-name-input').addEventListener('input', function() {
      document.getElementById('save-settings-btn').disabled = false;
    });
  });

  // Save settings (name update)
  document.getElementById('save-settings-btn').addEventListener('click', function() {
    var btn = this;
    var name = document.getElementById('org-name-input').value.trim();
    if (!name) { showAlert('alert-box', 'error', 'Organization name is required'); return; }

    btn.disabled = true;
    btn.textContent = 'Saving...';

    apiFetch('/api/v1/org', { method: 'PUT', body: { name: name } }).then(function(r) {
      btn.textContent = 'Save Changes';
      if (r.ok) {
        showAlert('alert-box', 'success', 'Settings saved');
      } else {
        btn.disabled = false;
        showAlert('alert-box', 'error', r.data.message || 'Failed to save');
      }
    }).catch(function() {
      btn.disabled = false;
      btn.textContent = 'Save Changes';
      showAlert('alert-box', 'error', 'Network error');
    });
  });

  // Delete org flow
  document.getElementById('delete-org-btn').addEventListener('click', function() {
    document.getElementById('confirm-slug').value = '';
    document.getElementById('confirm-delete').disabled = true;
    document.getElementById('delete-modal').classList.add('visible');
  });

  document.getElementById('cancel-delete').addEventListener('click', function() {
    document.getElementById('delete-modal').classList.remove('visible');
  });

  document.getElementById('confirm-slug').addEventListener('input', function() {
    document.getElementById('confirm-delete').disabled = this.value !== orgSlug;
  });

  document.getElementById('confirm-delete').addEventListener('click', function() {
    var btn = this;
    btn.disabled = true;
    btn.textContent = 'Deleting...';

    apiFetch('/api/v1/org', { method: 'DELETE' }).then(function(r) {
      if (r.ok) {
        try { localStorage.removeItem('savants_token'); } catch(e) {}
        window.location.href = '/?deleted=true';
      } else {
        btn.disabled = false;
        btn.textContent = 'Delete Forever';
        showAlert('alert-box', 'error', r.data.message || 'Failed to delete organization');
        document.getElementById('delete-modal').classList.remove('visible');
      }
    }).catch(function() {
      btn.disabled = false;
      btn.textContent = 'Delete Forever';
      showAlert('alert-box', 'error', 'Network error');
      document.getElementById('delete-modal').classList.remove('visible');
    });
  });

  // Close modals on overlay click
  document.querySelectorAll('.modal-overlay').forEach(function(overlay) {
    overlay.addEventListener('click', function(e) {
      if (e.target === overlay) overlay.classList.remove('visible');
    });
  });
})();
</script>
`;

  return layout("settings", "Settings", content) + js + closeHtml();
}

// ─── Docs Page ───────────────────────────────────────────────────────────────

export function docsPage(): string {
  const content = `
      <div id="alert-box" class="alert"></div>

      <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:24px;flex-wrap:wrap;gap:12px">
        <div>
          <div class="card-subtitle">Search and manage documentation sources. Certified docs are free. Upload private docs for your team.</div>
        </div>
      </div>

      <!-- Search -->
      <div class="card" style="margin-bottom:24px">
        <div style="display:flex;gap:12px;flex-wrap:wrap">
          <select id="doc-provider" class="form-select" style="width:180px">
            <option value="">All sources</option>
          </select>
          <input type="text" id="doc-search" class="form-input" placeholder="Search documentation..." style="flex:1;min-width:200px">
          <button class="btn btn-primary" id="search-btn">Search</button>
        </div>
        <div id="search-results" style="margin-top:16px"></div>
      </div>

      <!-- Certified Sources -->
      <div class="card" style="margin-bottom:24px">
        <div class="card-title">Certified Documentation</div>
        <div class="card-subtitle" style="margin-bottom:16px">Pre-indexed by Savants. Free to query. Updated automatically.</div>
        <div class="integration-grid" id="certified-docs">
          <div class="skeleton skeleton-row"></div>
          <div class="skeleton skeleton-row"></div>
        </div>
      </div>

      <!-- Private Docs -->
      <div class="card">
        <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:16px">
          <div>
            <div class="card-title">Private Documentation</div>
            <div class="card-subtitle">Your team's internal docs. Only visible to org members.</div>
          </div>
          <button class="btn btn-secondary btn-sm" id="upload-btn">Upload Docs</button>
        </div>
        <div id="private-docs">
          <div class="empty-state" style="padding:24px 0">
            <h3>No private docs yet</h3>
            <p>Upload markdown files or an OpenAPI spec to make them queryable.</p>
            <code style="display:block;margin-top:12px;color:var(--accent)">savants docs upload ./docs --project my-project</code>
          </div>
        </div>
      </div>
  `;

  const js = `
<script>
(function(){
  if (!getToken()) return;

  // Load certified docs
  fetch('https://api.savants.cloud/api/v1/docs').then(r=>r.json()).then(function(d) {
    var grid = document.getElementById('certified-docs');
    var select = document.getElementById('doc-provider');
    var providers = d.providers || [];

    grid.innerHTML = providers.map(function(p) {
      var statusClass = p.status === 'available' ? 'badge-green' : p.status === 'planned' ? 'badge-gray' : 'badge-cyan';
      var statusText = p.status === 'available' ? 'Indexed' : p.status === 'planned' ? 'Coming soon' : p.status;
      return '<div class="integration-card">' +
        '<div style="display:flex;justify-content:space-between;align-items:start">' +
        '<div><div class="integration-name">' + p.name + '</div>' +
        '<div style="color:var(--muted);font-size:0.8rem;margin-top:4px">' + p.description + '</div></div>' +
        '<span class="badge ' + statusClass + '">' + statusText + '</span>' +
        '</div>' +
        '<div style="margin-top:8px;font-size:0.8rem;color:var(--muted)">' +
        (p.versions > 0 ? p.versions + ' version(s) · Latest: ' + p.latest_version : '') +
        '</div></div>';
    }).join('');

    // Populate search dropdown
    providers.forEach(function(p) {
      var opt = document.createElement('option');
      opt.value = p.name;
      opt.textContent = p.name;
      select.appendChild(opt);
    });
  });

  // Search
  document.getElementById('search-btn').addEventListener('click', doSearch);
  document.getElementById('doc-search').addEventListener('keyup', function(e) {
    if (e.key === 'Enter') doSearch();
  });

  function doSearch() {
    var provider = document.getElementById('doc-provider').value;
    var query = document.getElementById('doc-search').value.trim();
    if (!query) return;

    var resultsDiv = document.getElementById('search-results');
    resultsDiv.innerHTML = '<div style="color:var(--muted)">Searching...</div>';

    if (!provider) {
      resultsDiv.innerHTML = '<div style="color:var(--muted)">Select a documentation source first.</div>';
      return;
    }

    fetch('https://api.savants.cloud/api/v1/docs/' + provider + '/search?q=' + encodeURIComponent(query))
      .then(function(r) { return r.json(); })
      .then(function(d) {
        if (d.total === 0) {
          resultsDiv.innerHTML = '<div style="color:var(--muted);padding:16px 0">No results for "' + query + '" in ' + provider + ' docs.</div>';
          return;
        }
        resultsDiv.innerHTML = d.results.map(function(r, i) {
          var section = r.matched_section;
          return '<div style="padding:12px 0;border-bottom:1px solid var(--border)">' +
            '<div style="font-weight:500">' + (i+1) + '. ' + r.title + '</div>' +
            (r.url ? '<a href="' + r.url + '" target="_blank" style="font-size:0.8rem;color:var(--accent)">' + r.url + '</a>' : '') +
            (section ? '<div style="color:var(--muted);font-size:0.85rem;margin-top:4px">' + (section.content || '').slice(0,200) + '</div>' : '') +
            '</div>';
        }).join('');
      });
  }

  // Load private docs
  apiFetch('/api/v1/projects').then(function(r) {
    if (!r.ok || !r.data.projects || r.data.projects.length === 0) return;
    var projectId = r.data.projects[0].id;

    fetch('https://api.savants.cloud/api/v1/docs/private?project_id=' + projectId, {
      headers: { 'Authorization': 'Bearer ' + getToken() }
    }).then(function(r) { return r.json(); }).then(function(d) {
      var docs = d.docs || [];
      if (docs.length === 0) return;

      var div = document.getElementById('private-docs');
      div.innerHTML = docs.map(function(doc) {
        var cfg = doc.config || {};
        return '<div style="padding:12px 0;border-bottom:1px solid var(--border);display:flex;justify-content:space-between;align-items:center">' +
          '<div>' +
          '<div style="font-weight:500">' + (cfg.name || 'Untitled') + '</div>' +
          '<div style="color:var(--muted);font-size:0.8rem">' + (doc.node_count || 0) + ' sections · ' + (cfg.format || 'markdown') + '</div>' +
          '</div>' +
          '<span class="badge badge-green">Indexed</span>' +
          '</div>';
      }).join('');
    });
  });
})();
</script>
`;

  return layout("docs", "Documentation", content) + js + closeHtml();
}

// ─── Page Router ─────────────────────────────────────────────────────────────

export function projectDetailPage(projectSlug: string): string {
  const content = `
      <div id="alert-box" class="alert"></div>
      <div style="margin-bottom:16px">
        <a href="/dashboard" style="color:var(--muted);text-decoration:none;font-size:0.85rem">< Back to Overview</a>
      </div>
      <div id="project-header" style="margin-bottom:24px">
        <h2 style="font-size:1.4rem;font-weight:700" id="project-name">Loading...</h2>
        <div style="color:var(--muted);font-size:0.85rem" id="project-slug">${projectSlug}</div>
      </div>

      <!-- Tabs -->
      <div style="display:flex;gap:8px;margin-bottom:24px;border-bottom:1px solid var(--border);padding-bottom:8px">
        <button class="btn btn-sm tab-btn active" data-tab="sources">Sources</button>
        <button class="btn btn-sm tab-btn" data-tab="members">Members</button>
        <button class="btn btn-sm tab-btn" data-tab="graph">Graph</button>
        <button class="btn btn-sm tab-btn" data-tab="errors">Errors</button>
        <button class="btn btn-sm tab-btn" data-tab="ci">CI Runs</button>
        <button class="btn btn-sm tab-btn" data-tab="settings">Settings</button>
      </div>

      <!-- Sources tab -->
      <div class="tab-content active" id="tab-sources">
        <div class="card" style="margin-bottom:16px">
          <div class="card-header">
            <div class="card-title">Connected Sources</div>
          </div>
          <div id="sources-list"><div class="skeleton" style="height:60px;border-radius:8px"></div></div>
        </div>
        <div class="card">
          <div class="card-header"><div class="card-title">Add Source</div></div>
          <div style="display:flex;gap:8px;flex-wrap:wrap;padding:8px 0">
            <button class="btn btn-secondary btn-sm" onclick="addSource('github_repo')">+ GitHub Repo</button>
            <button class="btn btn-secondary btn-sm" onclick="addSource('sentry_project')">+ Sentry Project</button>
            <button class="btn btn-secondary btn-sm" onclick="addSource('k8s_namespace')">+ K8s Namespace</button>
            <button class="btn btn-secondary btn-sm" onclick="addSource('slack_channel')">+ Slack Channel</button>
          </div>
        </div>
      </div>

      <!-- Members tab -->
      <div class="tab-content" id="tab-members" style="display:none">
        <div class="card" style="margin-bottom:16px">
          <div class="card-header"><div class="card-title">Team Members</div></div>
          <div id="members-list"><div class="skeleton" style="height:40px;border-radius:8px"></div></div>
        </div>
        <div class="card">
          <div class="card-header"><div class="card-title">Invite Member</div></div>
          <div style="display:flex;gap:8px;padding:8px 0">
            <input type="email" id="invite-email" placeholder="email@company.com" style="flex:1;background:var(--surface);border:1px solid var(--border);border-radius:6px;padding:8px 12px;color:var(--fg);font-size:0.85rem">
            <button class="btn btn-primary btn-sm" onclick="inviteMember()">Invite</button>
          </div>
        </div>
      </div>

      <!-- Graph tab -->
      <div class="tab-content" id="tab-graph" style="display:none">
        <div class="metrics-row" id="graph-metrics">
          <div class="metric-card"><div class="metric-value" id="graph-nodes">-</div><div class="metric-label">Functions</div></div>
          <div class="metric-card"><div class="metric-value" id="graph-edges">-</div><div class="metric-label">Call Chains</div></div>
          <div class="metric-card"><div class="metric-value" id="graph-files">-</div><div class="metric-label">Files</div></div>
          <div class="metric-card"><div class="metric-value" id="graph-last-indexed">-</div><div class="metric-label">Last Indexed</div></div>
        </div>
        <div class="card" style="margin-top:16px">
          <div class="card-header"><div class="card-title">Top Functions by Callers</div></div>
          <div id="graph-hotspots"><div style="color:var(--muted);font-size:0.85rem;padding:16px 0">Loading graph data...</div></div>
        </div>
      </div>

      <!-- Errors tab -->
      <div class="tab-content" id="tab-errors" style="display:none">
        <div class="card">
          <div class="card-header"><div class="card-title">Recent Sentry Errors</div></div>
          <div id="errors-list"><div style="color:var(--muted);font-size:0.85rem;padding:16px 0">Loading...</div></div>
        </div>
      </div>

      <!-- CI tab -->
      <div class="tab-content" id="tab-ci" style="display:none">
        <div class="card">
          <div class="card-header"><div class="card-title">Recent CI Runs</div></div>
          <div id="ci-list"><div style="color:var(--muted);font-size:0.85rem;padding:16px 0">Loading...</div></div>
        </div>
      </div>

      <!-- Settings tab -->
      <div class="tab-content" id="tab-settings" style="display:none">
        <div class="card">
          <div class="card-header"><div class="card-title">Project Settings</div></div>
          <div style="padding:16px 0">
            <div style="margin-bottom:16px">
              <label style="font-size:0.85rem;color:var(--muted);display:block;margin-bottom:4px">Project Name</label>
              <input type="text" id="settings-name" style="background:var(--surface);border:1px solid var(--border);border-radius:6px;padding:8px 12px;color:var(--fg);font-size:0.85rem;width:300px">
            </div>
            <button class="btn btn-primary btn-sm" onclick="saveSettings()">Save</button>
            <button class="btn btn-sm" style="background:var(--danger-bg);color:var(--danger);border:1px solid var(--danger-border);margin-left:16px" onclick="deleteProject()">Delete Project</button>
          </div>
        </div>
      </div>
  `;

  const js = `
<script>
(function(){
  if (!getToken()) return;
  var slug = '${projectSlug}';
  var projectId = '';

  // Tab switching
  document.querySelectorAll('.tab-btn').forEach(function(btn) {
    btn.addEventListener('click', function() {
      document.querySelectorAll('.tab-btn').forEach(function(b) { b.classList.remove('active'); });
      document.querySelectorAll('.tab-content').forEach(function(t) { t.style.display = 'none'; });
      btn.classList.add('active');
      document.getElementById('tab-' + btn.dataset.tab).style.display = 'block';
    });
  });

  // Load project list to find ID by slug
  apiFetch('/api/v1/projects').then(function(r) {
    if (!r.ok) return;
    var project = r.data.projects.find(function(p) { return p.slug === slug || p.name === slug; });
    if (!project) { showAlert('alert-box', 'error', 'Project not found'); return; }
    projectId = project.id;
    document.getElementById('project-name').textContent = project.name;
    document.getElementById('settings-name').value = project.name;

    // Load project detail
    apiFetch('/api/v1/projects/' + projectId).then(function(r) {
      if (!r.ok) return;
      var sources = r.data.sources || [];
      var members = r.data.members || [];

      // Sources
      var srcEl = document.getElementById('sources-list');
      if (sources.length === 0) {
        srcEl.innerHTML = '<div style="color:var(--muted);font-size:0.85rem;padding:16px 0">No sources connected. Add one below.</div>';
      } else {
        srcEl.innerHTML = sources.map(function(s) {
          var cfg = s.config || {};
          var detail = cfg.full_name || cfg.project_slug || cfg.namespace || cfg.channel_name || JSON.stringify(cfg).substring(0,60);
          return '<div class="activity-item" style="padding:10px 0;border-bottom:1px solid var(--border)">' +
            '<div class="status-dot green"></div>' +
            '<div style="flex:1"><div class="activity-text">' + s.source_type + '</div><div style="font-size:0.75rem;color:var(--muted)">' + detail + '</div></div>' +
            '<button class="btn btn-sm" style="font-size:0.7rem;padding:2px 8px;background:var(--danger-bg);color:var(--danger);border:1px solid var(--danger-border)" onclick="removeSource(\\'' + s.id + '\\')">Remove</button>' +
          '</div>';
        }).join('');
      }

      // Members
      var memEl = document.getElementById('members-list');
      if (members.length === 0) {
        memEl.innerHTML = '<div style="color:var(--muted);font-size:0.85rem;padding:16px 0">No members. Invite someone below.</div>';
      } else {
        memEl.innerHTML = members.map(function(m) {
          return '<div class="activity-item" style="padding:8px 0">' +
            '<div style="flex:1"><div class="activity-text">' + (m.email || m.name || '?') + '</div><div style="font-size:0.75rem;color:var(--muted)">' + (m.role || 'member') + '</div></div>' +
          '</div>';
        }).join('');
      }
    });

    // Load graph stats (use graph API directly, not tool call)
    apiFetch('/api/v1/graph/stats/' + projectId).then(function(r) {
      if (!r.ok || !r.data) return;
      var g = r.data;
      var totals = g.totals || {};
      document.getElementById('graph-nodes').textContent = formatNumber(totals.nodes || 0);
      document.getElementById('graph-edges').textContent = formatNumber(totals.edges || 0);

      // Count files from nodes_by_type
      var files = 0;
      (g.nodes_by_type || []).forEach(function(n) {
        if (n.type === 'file' || n.type === 'module') files += n.count;
      });
      document.getElementById('graph-files').textContent = formatNumber(files || totals.nodes || 0);
      document.getElementById('graph-last-indexed').textContent = 'Active';

      // Show top functions by type
      var hotspots = document.getElementById('graph-hotspots');
      if (g.nodes_by_type && g.nodes_by_type.length > 0) {
        hotspots.innerHTML = g.nodes_by_type.map(function(n) {
          return '<div class="activity-item" style="padding:6px 0"><div style="flex:1;font-size:0.85rem">' + n.type + '</div><div style="color:var(--accent);font-family:JetBrains Mono,monospace;font-size:0.85rem">' + n.count + '</div></div>';
        }).join('');
      } else {
        hotspots.innerHTML = '<div style="color:var(--muted);font-size:0.85rem;padding:16px 0">No graph data. Index a repo to see stats.</div>';
      }
    });
  });

  // Global functions for buttons
  window.addSource = function(type) {
    var value = prompt('Enter the ' + type.replace('_', ' ') + ' identifier:');
    if (!value) return;
    var config = {};
    if (type === 'github_repo') { var parts = value.split('/'); config = { owner: parts[0] || '', repo: parts[1] || value, full_name: value }; }
    else if (type === 'sentry_project') { config = { project_slug: value }; }
    else if (type === 'k8s_namespace') { config = { namespace: value }; }
    else if (type === 'slack_channel') { config = { channel_name: value }; }
    apiFetch('/api/v1/projects/' + projectId + '/sources', {
      method: 'POST', body: JSON.stringify({ source_type: type, config: config })
    }).then(function(r) { if (r.ok) location.reload(); else alert('Failed: ' + JSON.stringify(r.data)); });
  };

  window.removeSource = function(sourceId) {
    if (!confirm('Remove this source?')) return;
    apiFetch('/api/v1/projects/' + projectId + '/sources/' + sourceId, { method: 'DELETE' })
      .then(function(r) { if (r.ok) location.reload(); });
  };

  window.inviteMember = function() {
    var email = document.getElementById('invite-email').value.trim();
    if (!email) return;
    apiFetch('/api/v1/projects/' + projectId + '/members', {
      method: 'POST', body: JSON.stringify({ email: email })
    }).then(function(r) { if (r.ok) location.reload(); else alert(r.data.message || 'Failed'); });
  };

  window.saveSettings = function() {
    var name = document.getElementById('settings-name').value.trim();
    if (!name) return;
    // TODO: API to update project name
    alert('Saved (not implemented yet)');
  };

  window.deleteProject = function() {
    if (!confirm('Delete this project? This cannot be undone.')) return;
    apiFetch('/api/v1/projects/' + projectId, { method: 'DELETE' })
      .then(function(r) { if (r.ok) location.href = '/dashboard'; });
  };
})();
</script>
`;

  return layout("overview", "Project: " + projectSlug, content) + js + closeHtml();
}

export function incidentsPage(): string {
  const content = `
      <div id="alert-box" class="alert"></div>
      <div style="margin-bottom:24px">
        <div class="card-subtitle">Active and recent incidents with causal chain analysis.</div>
      </div>

      <!-- Active Incidents -->
      <div class="card" style="margin-bottom:24px">
        <div class="card-header">
          <div class="card-title" style="color:var(--danger)">Active Incidents</div>
        </div>
        <div id="active-incidents">
          <div class="skeleton skeleton-row"></div>
        </div>
      </div>

      <!-- Incident Timeline -->
      <div class="card" style="margin-bottom:24px">
        <div class="card-header">
          <div class="card-title">Timeline (last 2 hours)</div>
        </div>
        <div id="incident-timeline" style="position:relative;min-height:200px;padding:16px 0">
          <div class="skeleton skeleton-row"></div>
        </div>
      </div>

      <!-- Causal Chain -->
      <div class="card" style="margin-bottom:24px">
        <div class="card-header">
          <div class="card-title">Root Cause Analysis</div>
        </div>
        <div id="causal-chain">
          <div style="padding:24px;text-align:center;color:var(--muted);font-size:0.85rem">
            Select an incident above to see its causal chain.
          </div>
        </div>
      </div>

      <!-- Recently Resolved -->
      <div class="card">
        <div class="card-header">
          <div class="card-title">Recently Resolved</div>
        </div>
        <div id="resolved-incidents">
          <div class="skeleton skeleton-row"></div>
        </div>
      </div>
  `;

  const js = `
<style>
  .timeline-event { display:flex; align-items:flex-start; gap:12px; padding:10px 0; border-left:2px solid var(--border); margin-left:8px; padding-left:20px; position:relative; cursor:pointer; transition: background 0.15s; border-radius:0 8px 8px 0; }
  .timeline-event:hover { background:rgba(34,211,238,0.04); }
  .timeline-event::before { content:''; position:absolute; left:-6px; top:14px; width:10px; height:10px; border-radius:50%; border:2px solid var(--border); background:var(--bg); }
  .timeline-event.critical::before { border-color:var(--danger); background:var(--danger); }
  .timeline-event.warning::before { border-color:var(--warning); background:var(--warning); }
  .timeline-event.info::before { border-color:var(--accent); background:var(--accent); }
  .timeline-event .ev-time { font-family:'JetBrains Mono',monospace; font-size:0.72rem; color:var(--muted); min-width:50px; }
  .timeline-event .ev-title { font-size:0.85rem; font-weight:500; }
  .timeline-event .ev-meta { font-size:0.72rem; color:var(--muted); margin-top:2px; }
  .incident-card { padding:16px; border:1px solid var(--border); border-radius:10px; margin-bottom:12px; cursor:pointer; transition: border-color 0.2s; }
  .incident-card:hover { border-color:var(--accent); }
  .incident-card.critical { border-left:3px solid var(--danger); }
  .incident-card.warning { border-left:3px solid var(--warning); }
  .causal-node { display:flex; align-items:center; gap:12px; padding:12px 16px; border:1px solid var(--border); border-radius:8px; margin-bottom:8px; position:relative; }
  .causal-node::after { content:''; position:absolute; bottom:-9px; left:24px; width:2px; height:8px; background:var(--border); }
  .causal-node:last-child::after { display:none; }
  .causal-node .cn-score { font-family:'JetBrains Mono',monospace; font-size:0.75rem; padding:2px 8px; border-radius:4px; background:var(--surface); color:var(--accent); }
  .resolved-item { display:flex; align-items:center; justify-content:space-between; padding:10px 0; border-bottom:1px solid var(--border); }
  .resolved-item:last-child { border-bottom:none; }
</style>
<script>
(function(){
  if (!getToken()) return;

  // Load active incidents
  apiFetch('/api/v1/agents/incidents').then(function(r) {
    if (!r.ok) return;
    var active = r.data.active || [];
    var resolved = r.data.resolved || [];

    // Active
    var el = document.getElementById('active-incidents');
    if (active.length === 0) {
      el.innerHTML = '<div style="padding:24px;text-align:center;color:var(--success);font-size:0.9rem">No active incidents. All systems operational.</div>';
    } else {
      el.innerHTML = active.map(function(inc) {
        return '<div class="incident-card ' + inc.severity + '" onclick="loadCauses(\\'' + inc.key + '\\')">' +
          '<div style="display:flex;justify-content:space-between;align-items:center">' +
            '<div style="font-weight:600;font-size:0.9rem">' + inc.title.substring(0,60) + '</div>' +
            '<div style="font-size:0.72rem;color:var(--muted)">' + inc.occurrences + 'x / ' + inc.duration_min + 'min</div>' +
          '</div>' +
          '<div style="font-size:0.78rem;color:var(--muted);margin-top:4px">' + inc.category + ' - ' + inc.agent + '</div>' +
        '</div>';
      }).join('');
    }

    // Resolved
    var rel = document.getElementById('resolved-incidents');
    if (resolved.length === 0) {
      rel.innerHTML = '<div style="padding:16px;text-align:center;color:var(--muted);font-size:0.85rem">No recently resolved incidents.</div>';
    } else {
      rel.innerHTML = resolved.slice(0,10).map(function(inc) {
        return '<div class="resolved-item">' +
          '<div><div style="font-size:0.85rem">' + inc.title.substring(0,50) + '</div>' +
          '<div style="font-size:0.72rem;color:var(--muted)">' + inc.category + '</div></div>' +
          '<div style="font-size:0.72rem;color:var(--success)">resolved ' + (inc.resolved_min_ago || '?') + 'min ago</div>' +
        '</div>';
      }).join('');
    }
  });

  // Load timeline
  apiFetch('/api/v1/agents/events?limit=30').then(function(r) {
    if (!r.ok || !r.data.events) return;
    var el = document.getElementById('incident-timeline');
    var events = r.data.events.filter(function(e) { return e.severity !== 'info'; });
    if (events.length === 0) {
      el.innerHTML = '<div style="padding:24px;text-align:center;color:var(--muted);font-size:0.85rem">No warning/critical events in the last 2 hours.</div>';
      return;
    }
    el.innerHTML = events.map(function(e) {
      var ts = new Date(e.timestamp * 1000);
      var time = ts.getHours().toString().padStart(2,'0') + ':' + ts.getMinutes().toString().padStart(2,'0');
      return '<div class="timeline-event ' + e.severity + '">' +
        '<div class="ev-time">' + time + '</div>' +
        '<div><div class="ev-title">' + (e.title || '').substring(0,60) + '</div>' +
        '<div class="ev-meta">' + (e.agent || '') + ' - ' + (e.category || '') + '</div></div>' +
      '</div>';
    }).join('');
  });
})();

// Load causal chain for an incident
function loadCauses(key) {
  apiFetch('/api/v1/tools/call', {
    method: 'POST',
    body: JSON.stringify({tool: 'find_causes', input: {event_type: 'infrastructure', lookback_minutes: 120}})
  }).then(function(r) {
    var el = document.getElementById('causal-chain');
    if (!r.ok || !r.data.result) {
      el.innerHTML = '<div style="padding:16px;color:var(--muted)">Could not load causal analysis.</div>';
      return;
    }
    var result = r.data.result;
    var causes = result.probable_causes || [];
    if (causes.length === 0) {
      el.innerHTML = '<div style="padding:16px;color:var(--muted)">No causal data available for this incident.</div>';
      return;
    }
    el.innerHTML = '<div style="padding:4px 0;margin-bottom:12px;font-size:0.82rem;color:var(--muted)">Confidence: ' + ((result.confidence || 0) * 100).toFixed(0) + '% - ' + (result.reasoning || '') + '</div>' +
      causes.slice(0,8).map(function(c) {
        return '<div class="causal-node">' +
          '<div class="cn-score">' + (c.causal_score || 0).toFixed(2) + '</div>' +
          '<div style="flex:1"><div style="font-size:0.85rem;font-weight:500">' + (c.event_title || '?').substring(0,50) + '</div>' +
          '<div style="font-size:0.72rem;color:var(--muted)">' + (c.explanation || '').substring(0,80) + '</div></div>' +
        '</div>';
      }).join('');
  });
}
</script>
`;

  return layout("incidents", "Incidents", content) + js + closeHtml();
}

export function dashboardPage(page?: string): string {
  switch (page) {
    case "keys":
      return keysPage();
    case "team":
      return teamPage();
    case "integrations":
      return integrationsPage();
    case "incidents":
      return incidentsPage();
    case "docs":
      return docsPage();
    case "billing":
      return billingPage();
    case "settings":
      return settingsPage();
    default:
      return overviewPage();
  }
}
