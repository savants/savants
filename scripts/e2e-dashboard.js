// Savants E2E Dashboard Test Suite
// Playwright-based. Tests every page, link, button, and API call.
// Run: node scripts/e2e-dashboard.js
// Output: /tmp/e2e-results.json + screenshots in /tmp/e2e-*.png

const { chromium } = require('playwright');
const fs = require('fs');

const BASE = 'https://savants.cloud';
const SITE = 'https://savants.dev';
const API = 'https://api.savants.cloud/api/v1';
const SCREENSHOTS_DIR = '/tmp';

const results = { passed: 0, failed: 0, skipped: 0, tests: [] };

function log(status, name, detail) {
  const icon = status === 'PASS' ? '\x1b[32mPASS\x1b[0m' : status === 'FAIL' ? '\x1b[31mFAIL\x1b[0m' : '\x1b[33mSKIP\x1b[0m';
  console.log(`  ${icon} ${name}${detail ? ' - ' + detail : ''}`);
  results.tests.push({ name, status, detail: detail || '' });
  if (status === 'PASS') results.passed++;
  else if (status === 'FAIL') results.failed++;
  else results.skipped++;
}

async function screenshot(page, name) {
  await page.screenshot({ path: `${SCREENSHOTS_DIR}/e2e-${name}.png`, fullPage: true });
}

async function authenticate() {
  // Device flow: create code, auto-approve via D1, get token
  const codeRes = await fetch(`${API.replace('/api/v1', '')}/auth/device/code`, { method: 'POST' });
  const { device_code } = await codeRes.json();

  // Auto-approve (uses D1 direct API - same as qa-screenshots.js)
  await fetch('https://api.cloudflare.com/client/v4/accounts/4992fd600f9894326a82a0f8573a7c38/d1/database/bf5c1140-48ac-4b61-bb5c-6fc2a673eb2d/query', {
    method: 'POST',
    headers: { 'Authorization': 'Bearer bSnXmjhm8PJOAtHG2-_X5FKl6G0-9g7dQUl4TgwF', 'Content-Type': 'application/json' },
    body: JSON.stringify({ sql: `UPDATE device_auth_sessions SET status = 'approved', user_id = '139a5530-cf8c-4389-880b-c15608980c28', org_id = 'cb198567-f0ee-43e5-a1c0-359fd51f9e99' WHERE device_code = '${device_code}'` })
  });
  await new Promise(r => setTimeout(r, 1500));
  const tokenRes = await fetch(`${API.replace('/api/v1', '')}/auth/device/token`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ device_code })
  });
  const { access_token } = await tokenRes.json();
  return access_token;
}

(async () => {
  console.log('\n  Savants E2E Test Suite\n');
  const startTime = Date.now();

  // ── Auth ──
  let token;
  try {
    token = await authenticate();
    log(token ? 'PASS' : 'FAIL', 'Auth: device flow', token ? 'token obtained' : 'no token');
  } catch (e) {
    log('FAIL', 'Auth: device flow', e.message);
    console.log('\n  Cannot proceed without auth. Exiting.\n');
    return;
  }

  // ── API Tests ──
  const apiTests = [
    { name: 'API: projects list', path: '/projects', check: (d) => Array.isArray(d.projects) },
    { name: 'API: agents list', path: '/agents', check: (d) => Array.isArray(d.agents) },
    { name: 'API: integrations', path: '/integrations', check: (d) => Array.isArray(d.integrations) },
    { name: 'API: billing', path: '/billing', check: (d) => d.plan !== undefined },
    { name: 'API: usage', path: '/usage', check: (d) => d.total_calls !== undefined },
    { name: 'API: events', path: '/agents/events?limit=5', check: (d) => Array.isArray(d.events) },
    { name: 'API: incidents', path: '/agents/incidents', check: (d) => d.active !== undefined },
  ];

  for (const t of apiTests) {
    try {
      const res = await fetch(API + t.path, { headers: { Authorization: 'Bearer ' + token } });
      const data = await res.json();
      log(res.ok && t.check(data) ? 'PASS' : 'FAIL', t.name, `${res.status} ${JSON.stringify(data).substring(0, 80)}`);
    } catch (e) {
      log('FAIL', t.name, e.message);
    }
  }

  // ── Browser Tests ──
  const browser = await chromium.launch({
    headless: true,
    executablePath: process.env.CHROME_PATH || require('child_process').execSync('find /nix/store -maxdepth 3 -name "chromium" -path "*/bin/*" 2>/dev/null | head -1').toString().trim() || '/usr/bin/chromium',
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });
  const context = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await context.newPage();

  // Inject auth token
  async function goAuth(url) {
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 15000 });
    await page.evaluate((t) => { localStorage.setItem('savants_token', t); }, token);
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 15000 });
    await page.waitForTimeout(3000); // let JS render
  }

  // ── Public Pages ──
  const publicPages = [
    { url: SITE + '/', name: 'site-home', check: 'savants' },
    { url: SITE + '/pricing', name: 'site-pricing', check: '499' },
    { url: SITE + '/install', name: 'site-install', check: 'curl' },
    { url: SITE + '/privacy', name: 'site-privacy', check: 'Privacy' },
    { url: SITE + '/terms', name: 'site-terms', check: 'Terms' },
  ];

  for (const p of publicPages) {
    try {
      await page.goto(p.url, { waitUntil: 'domcontentloaded', timeout: 10000 });
      const content = await page.textContent('body');
      const found = content.includes(p.check);
      await screenshot(page, p.name);
      log(found ? 'PASS' : 'FAIL', `Page: ${p.name}`, found ? 'content OK' : `missing "${p.check}"`);
    } catch (e) {
      log('FAIL', `Page: ${p.name}`, e.message.substring(0, 80));
    }
  }

  // ── Dashboard Pages ──
  const dashPages = [
    { url: '/dashboard', name: 'dashboard-overview', check: 'health-badge' },
    { url: '/dashboard/keys', name: 'dashboard-keys', check: 'API Key' },
    { url: '/dashboard/team', name: 'dashboard-team', check: 'Team' },
    { url: '/dashboard/integrations', name: 'dashboard-integrations', check: 'ntegration' },
    { url: '/dashboard/billing', name: 'dashboard-billing', check: 'Billing' },
    { url: '/dashboard/settings', name: 'dashboard-settings', check: 'Settings' },
  ];

  for (const p of dashPages) {
    try {
      await goAuth(BASE + p.url);
      const html = await page.content();
      const found = html.includes(p.check);
      await screenshot(page, p.name);
      log(found ? 'PASS' : 'FAIL', `Dashboard: ${p.name}`, found ? 'rendered OK' : `missing "${p.check}"`);
    } catch (e) {
      log('FAIL', `Dashboard: ${p.name}`, e.message.substring(0, 80));
    }
  }

  // ── Health Badge Check ──
  try {
    await goAuth(BASE + '/dashboard');
    const badgeText = await page.textContent('#health-title');
    log(badgeText && badgeText.length > 0 ? 'PASS' : 'FAIL', 'Dashboard: health badge renders', badgeText);
  } catch (e) {
    log('FAIL', 'Dashboard: health badge renders', e.message.substring(0, 80));
  }

  // ── Agent Dedup Check ──
  try {
    const agentsList = await page.textContent('#agents-list');
    const astraCount = (agentsList.match(/astra/g) || []).length;
    log(astraCount <= 1 ? 'PASS' : 'FAIL', 'Dashboard: agent dedup', `"astra" appears ${astraCount} time(s)`);
  } catch (e) {
    log('FAIL', 'Dashboard: agent dedup', e.message.substring(0, 80));
  }

  // ── Sidebar Links ──
  const sidebarLinks = await page.$$eval('.nav-item', els => els.map(e => ({ href: e.getAttribute('href'), text: e.textContent.trim() })));
  for (const link of sidebarLinks) {
    if (!link.href || link.href === '#') continue;
    try {
      await goAuth(BASE + link.href);
      const status = await page.evaluate(() => document.readyState);
      await screenshot(page, 'nav-' + link.text.toLowerCase().replace(/\s+/g, '-'));
      log(status === 'complete' ? 'PASS' : 'FAIL', `Nav link: ${link.text}`, link.href);
    } catch (e) {
      log('FAIL', `Nav link: ${link.text}`, e.message.substring(0, 80));
    }
  }

  // ── Pricing Page Buttons ──
  try {
    await page.goto(SITE + '/pricing', { waitUntil: 'domcontentloaded', timeout: 10000 });
    const buttons = await page.$$eval('.plan-cta', els => els.map(e => ({ text: e.textContent.trim(), tag: e.tagName })));
    log(buttons.length >= 4 ? 'PASS' : 'FAIL', 'Pricing: CTA buttons present', `${buttons.length} buttons found`);
  } catch (e) {
    log('FAIL', 'Pricing: CTA buttons present', e.message.substring(0, 80));
  }

  await browser.close();

  // ── Results ──
  const elapsed = ((Date.now() - startTime) / 1000).toFixed(1);
  console.log(`\n  Results: ${results.passed} passed, ${results.failed} failed, ${results.skipped} skipped (${elapsed}s)\n`);
  fs.writeFileSync('/tmp/e2e-results.json', JSON.stringify(results, null, 2));
  console.log(`  Screenshots: ${SCREENSHOTS_DIR}/e2e-*.png`);
  console.log(`  Results: /tmp/e2e-results.json\n`);

  process.exit(results.failed > 0 ? 1 : 0);
})();
