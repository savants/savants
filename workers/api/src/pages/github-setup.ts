/**
 * GitHub integration setup page.
 * After OAuth, user lands here to pick repos and assign them to projects.
 */

export function githubSetupPage(status?: string, message?: string): string {
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>GitHub Integration - Savants</title>
<link rel="icon" type="image/svg+xml" href="https://savants.dev/favicon.svg">
<link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
<style>
*{margin:0;padding:0;box-sizing:border-box}
body{font-family:'Inter',system-ui,sans-serif;background:#0a0a0a;color:#e5e5e5;min-height:100vh;padding:0}
a{color:#22d3ee;text-decoration:none}
.page{max-width:800px;margin:0 auto;padding:40px 24px 80px}
.back{display:inline-flex;align-items:center;gap:6px;color:#737373;font-size:0.85rem;margin-bottom:24px;text-decoration:none}
.back:hover{color:#e5e5e5}
h1{font-size:1.5rem;font-weight:700;margin-bottom:8px}
.subtitle{color:#737373;font-size:0.95rem;margin-bottom:32px}
.card{background:#141414;border:1px solid #262626;border-radius:14px;padding:24px;margin-bottom:20px}
.card h2{font-size:1.1rem;font-weight:600;margin-bottom:16px}
.status-bar{display:flex;align-items:center;gap:10px;padding:14px 18px;border-radius:10px;margin-bottom:24px;font-size:0.9rem}
.status-bar.success{background:#052e16;border:1px solid #166534;color:#4ade80}
.status-bar.error{background:#2a0a0a;border:1px solid #7f1d1d;color:#f87171}
.status-bar.info{background:#0c1a2e;border:1px solid #1e3a5f;color:#60a5fa}
.repo-list{display:flex;flex-direction:column;gap:8px}
.repo-item{display:flex;align-items:center;gap:12px;padding:12px 16px;border:1px solid #262626;border-radius:10px;cursor:pointer;transition:border-color 0.15s,background 0.15s}
.repo-item:hover{border-color:#333;background:rgba(255,255,255,0.02)}
.repo-item.selected{border-color:#22d3ee;background:rgba(34,211,238,0.05)}
.repo-item input[type=checkbox]{accent-color:#22d3ee;width:18px;height:18px;cursor:pointer}
.repo-name{font-weight:500;font-size:0.95rem;flex:1}
.repo-meta{display:flex;gap:12px;align-items:center}
.repo-badge{font-size:0.7rem;padding:2px 8px;border-radius:6px;font-weight:500}
.repo-badge.private{background:#2a0a2a;color:#c084fc;border:1px solid #581c87}
.repo-badge.public{background:#052e16;color:#4ade80;border:1px solid #166534}
.repo-lang{font-size:0.8rem;color:#737373;font-family:'JetBrains Mono',monospace}
.project-select{margin-top:12px;padding:10px 14px;background:#1e1e1e;border:1px solid #333;border-radius:8px;color:#e5e5e5;font-size:0.85rem;width:100%}
.project-select option{background:#1e1e1e;color:#e5e5e5}
.btn{display:inline-flex;align-items:center;gap:8px;padding:10px 20px;border-radius:8px;font-size:0.875rem;font-weight:600;cursor:pointer;border:none;transition:all 0.15s}
.btn-primary{background:linear-gradient(135deg,#22d3ee,#a78bfa);color:#0a0a0a}
.btn-primary:hover{transform:translateY(-1px);box-shadow:0 4px 20px rgba(34,211,238,0.3)}
.btn-primary:disabled{opacity:0.5;cursor:not-allowed;transform:none;box-shadow:none}
.btn-secondary{background:transparent;border:1px solid #333;color:#e5e5e5}
.actions{display:flex;gap:12px;margin-top:24px;justify-content:flex-end}
.loading{color:#737373;padding:40px;text-align:center}
.search{width:100%;padding:10px 14px;background:#1e1e1e;border:1px solid #333;border-radius:8px;color:#e5e5e5;font-size:0.9rem;margin-bottom:16px}
.search:focus{outline:none;border-color:#22d3ee}
.section-label{font-size:0.75rem;font-weight:600;color:#737373;text-transform:uppercase;letter-spacing:0.05em;margin:16px 0 8px}
.graph-toggle{display:flex;align-items:center;justify-content:space-between;padding:12px 16px;border:1px solid #262626;border-radius:10px;margin-top:12px}
.graph-toggle label{font-size:0.85rem;color:#737373}
.toggle{position:relative;width:40px;height:22px;cursor:pointer}
.toggle input{opacity:0;width:0;height:0}
.toggle .slider{position:absolute;top:0;left:0;right:0;bottom:0;background:#333;border-radius:11px;transition:background 0.2s}
.toggle .slider:before{content:'';position:absolute;height:16px;width:16px;left:3px;bottom:3px;background:#e5e5e5;border-radius:50%;transition:transform 0.2s}
.toggle input:checked+.slider{background:#22d3ee}
.toggle input:checked+.slider:before{transform:translateX(18px)}
.empty{text-align:center;padding:40px;color:#737373}
.count{font-size:0.8rem;color:#737373;margin-top:8px}
@media(max-width:768px){
  .page{padding:20px 16px 60px}
  .repo-meta{flex-direction:column;align-items:flex-start;gap:4px}
  .actions{flex-direction:column}
  .btn{width:100%;justify-content:center}
}
</style>
</head>
<body>
<div class="page">
  <a href="/dashboard/integrations" class="back">&larr; Back to Integrations</a>
  <h1>GitHub Integration</h1>
  <p class="subtitle">Select repositories to index. Savants will build a code graph for each selected repo.</p>

  ${status === 'success' ? '<div class="status-bar success">GitHub connected successfully. Select repos to index below.</div>' : ''}
  ${status === 'error' ? `<div class="status-bar error">${message || 'Connection failed'}</div>` : ''}

  <div id="content">
    <div class="loading">Loading repositories...</div>
  </div>
</div>

<script>
(function(){
  // Get token from URL or localStorage
  var params = new URLSearchParams(location.search);
  var token = params.get('token') || localStorage.getItem('savants_token');
  if (params.get('token')) {
    localStorage.setItem('savants_token', token);
    history.replaceState({}, '', location.pathname + (params.get('status') ? '?status=' + params.get('status') : ''));
  }
  if (!token) {
    document.getElementById('content').innerHTML = '<div class="status-bar error">Not authenticated. <a href="/activate">Sign in first</a></div>';
    return;
  }

  function api(path, opts) {
    return fetch('https://api.savants.cloud' + path, Object.assign({
      headers: { 'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json' }
    }, opts)).then(function(r) { return r.json(); });
  }

  // Load repos and projects in parallel
  Promise.all([
    api('/api/v1/projects/github/repos'),
    api('/api/v1/projects')
  ]).then(function(results) {
    var repoData = results[0];
    var projectData = results[1];
    var repos = repoData.repos || [];
    var projects = projectData.projects || [];

    if (repoData.error) {
      document.getElementById('content').innerHTML =
        '<div class="status-bar error">' + (repoData.message || 'Failed to load repos') + '</div>' +
        '<a href="/auth/github?redirect=https://savants.cloud/integrations/github" class="btn btn-primary">Connect GitHub</a>';
      return;
    }

    if (repos.length === 0) {
      document.getElementById('content').innerHTML = '<div class="empty"><h3>No repositories found</h3><p>Make sure your GitHub account has access to repositories.</p></div>';
      return;
    }

    var selectedRepos = {};

    var html = '<input type="text" class="search" id="repo-search" placeholder="Search repositories...">';
    html += '<div class="count" id="repo-count">' + repos.length + ' repositories</div>';
    html += '<div class="repo-list" id="repo-list">';

    repos.forEach(function(repo) {
      var badge = repo.private
        ? '<span class="repo-badge private">Private</span>'
        : '<span class="repo-badge public">Public</span>';
      var lang = repo.language ? '<span class="repo-lang">' + repo.language + '</span>' : '';

      html += '<label class="repo-item" data-name="' + repo.full_name.toLowerCase() + '">' +
        '<input type="checkbox" value="' + repo.full_name + '" data-owner="' + repo.owner + '" data-repo="' + repo.name + '" data-branch="' + repo.default_branch + '">' +
        '<div class="repo-name">' + repo.full_name + '</div>' +
        '<div class="repo-meta">' + lang + badge + '</div>' +
        '</label>';
    });
    html += '</div>';

    // Project assignment
    html += '<div class="card" style="margin-top:24px">';
    html += '<h2>Assign to Project</h2>';
    html += '<p style="color:#737373;font-size:0.85rem;margin-bottom:12px">Selected repos will be added as sources to this project. Data is isolated per project.</p>';

    if (projects.length > 0) {
      html += '<select class="project-select" id="project-select">';
      projects.forEach(function(p) {
        html += '<option value="' + p.id + '">' + p.name + '</option>';
      });
      html += '<option value="__new">+ Create new project</option>';
      html += '</select>';
    } else {
      html += '<input type="text" class="search" id="new-project-name" placeholder="Project name (e.g. Backend API)">';
    }

    html += '<div class="graph-toggle">';
    html += '<label>Enable code graph (index functions, call chains, imports)</label>';
    html += '<label class="toggle"><input type="checkbox" id="graph-enabled" checked><span class="slider"></span></label>';
    html += '</div>';

    html += '<div class="actions">';
    html += '<a href="/dashboard/integrations" class="btn btn-secondary">Cancel</a>';
    html += '<button class="btn btn-primary" id="save-btn" disabled>Add Selected Repos</button>';
    html += '</div>';
    html += '</div>';

    document.getElementById('content').innerHTML = html;

    // Search filter
    document.getElementById('repo-search').addEventListener('input', function(e) {
      var query = e.target.value.toLowerCase();
      document.querySelectorAll('.repo-item').forEach(function(item) {
        item.style.display = item.dataset.name.includes(query) ? '' : 'none';
      });
    });

    // Track selections
    document.querySelectorAll('.repo-item input[type=checkbox]').forEach(function(cb) {
      cb.addEventListener('change', function() {
        var item = cb.closest('.repo-item');
        if (cb.checked) {
          item.classList.add('selected');
          selectedRepos[cb.value] = {
            owner: cb.dataset.owner,
            repo: cb.dataset.repo,
            branch: cb.dataset.branch,
            full_name: cb.value
          };
        } else {
          item.classList.remove('selected');
          delete selectedRepos[cb.value];
        }
        var count = Object.keys(selectedRepos).length;
        document.getElementById('save-btn').disabled = count === 0;
        document.getElementById('save-btn').textContent = count > 0
          ? 'Add ' + count + ' Repo' + (count > 1 ? 's' : '')
          : 'Add Selected Repos';
      });
    });

    // New project input
    var projectSelect = document.getElementById('project-select');
    if (projectSelect) {
      projectSelect.addEventListener('change', function() {
        if (projectSelect.value === '__new') {
          var input = document.createElement('input');
          input.type = 'text';
          input.className = 'search';
          input.id = 'new-project-name';
          input.placeholder = 'Project name';
          input.style.marginTop = '12px';
          projectSelect.parentNode.insertBefore(input, projectSelect.nextSibling);
        } else {
          var existing = document.getElementById('new-project-name');
          if (existing) existing.remove();
        }
      });
    }

    // Save
    document.getElementById('save-btn').addEventListener('click', function() {
      var btn = document.getElementById('save-btn');
      btn.disabled = true;
      btn.textContent = 'Adding...';

      var repoList = Object.values(selectedRepos);
      var graphEnabled = document.getElementById('graph-enabled').checked;

      // Get or create project
      var projectPromise;
      var projectSelect = document.getElementById('project-select');
      var newName = document.getElementById('new-project-name');

      if (newName && newName.value) {
        projectPromise = api('/api/v1/projects', {
          method: 'POST',
          body: JSON.stringify({ name: newName.value })
        }).then(function(r) { return r.id; });
      } else if (projectSelect && projectSelect.value !== '__new') {
        projectPromise = Promise.resolve(projectSelect.value);
      } else {
        projectPromise = api('/api/v1/projects', {
          method: 'POST',
          body: JSON.stringify({ name: repoList[0].repo })
        }).then(function(r) { return r.id; });
      }

      projectPromise.then(function(projectId) {
        // Add each repo as a source
        return Promise.all(repoList.map(function(repo) {
          return api('/api/v1/projects/' + projectId + '/sources', {
            method: 'POST',
            body: JSON.stringify({
              source_type: 'github_repo',
              config: {
                owner: repo.owner,
                repo: repo.repo,
                full_name: repo.full_name,
                branch: repo.branch,
                graph_enabled: graphEnabled
              }
            })
          });
        }));
      }).then(function() {
        location.href = '/dashboard/integrations?status=success&message=Repos+added+successfully';
      }).catch(function(err) {
        btn.disabled = false;
        btn.textContent = 'Add Selected Repos';
        alert('Failed: ' + (err.message || err));
      });
    });
  });
})();
</script>
</body>
</html>`;
}
