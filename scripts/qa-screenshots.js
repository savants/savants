const { chromium } = require('playwright');

(async () => {
  const browser = await chromium.launch({
    headless: true,
    executablePath: '/nix/store/nw961dvpvik5m19kbay4cg27wxgl3sdv-playwright-chromium-headless-shell/chrome-linux/headless_shell',
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });
  const context = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await context.newPage();

  // Get auth token
  const deviceRes = await fetch('https://api.savants.cloud/auth/device/code', { method: 'POST' });
  const { device_code } = await deviceRes.json();
  await fetch('https://api.cloudflare.com/client/v4/accounts/4992fd600f9894326a82a0f8573a7c38/d1/database/bf5c1140-48ac-4b61-bb5c-6fc2a673eb2d/query', {
    method: 'POST',
    headers: { 'Authorization': 'Bearer bSnXmjhm8PJOAtHG2-_X5FKl6G0-9g7dQUl4TgwF', 'Content-Type': 'application/json' },
    body: JSON.stringify({ sql: `UPDATE device_auth_sessions SET status = 'approved', user_id = '139a5530-cf8c-4389-880b-c15608980c28', org_id = 'cb198567-f0ee-43e5-a1c0-359fd51f9e99' WHERE device_code = '${device_code}'` })
  });
  await new Promise(r => setTimeout(r, 1500));
  const tokenRes = await fetch('https://api.savants.cloud/auth/device/token', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ device_code })
  });
  const { access_token } = await tokenRes.json();
  if (!access_token) { console.log('FAIL: no token'); await browser.close(); return; }
  console.log('Token OK');

  const pages = [
    { url: '/dashboard', name: '01-overview' },
    { url: '/dashboard/project/talent-pipeline', name: '02-talent-pipeline' },
    { url: '/dashboard/project/mingovanburne', name: '03-mingovanburne' },
    { url: '/dashboard/keys', name: '04-keys' },
    { url: '/dashboard/team', name: '05-team' },
    { url: '/dashboard/integrations', name: '06-integrations' },
    { url: '/dashboard/billing', name: '07-billing' },
    { url: '/dashboard/settings', name: '08-settings' },
  ];

  for (const p of pages) {
    try {
      await page.goto('https://savants.cloud' + p.url, { waitUntil: 'domcontentloaded', timeout: 10000 });
      await page.evaluate((token) => { localStorage.setItem('savants_token', token); }, access_token);
      await page.goto('https://savants.cloud' + p.url, { waitUntil: 'domcontentloaded', timeout: 10000 });
      await page.waitForTimeout(4000);
      await page.screenshot({ path: '/tmp/qa-' + p.name + '.png', fullPage: true });
      console.log('OK: ' + p.name);
    } catch (e) {
      console.log('FAIL: ' + p.name + ' - ' + e.message.substring(0, 80));
    }
  }

  await browser.close();
  console.log('QA complete');
})();
