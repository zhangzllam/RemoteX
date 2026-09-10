// Browser contract/visual tests: mocked Tauri API, never a production password.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const path = require('node:path');
const fs = require('node:fs');
const assert = require('node:assert/strict');
const output = path.resolve(__dirname, '../../../../target/ui-refinement');
fs.mkdirSync(output, { recursive: true });

(async () => {
  const browser = await chromium.launch({ headless: true, channel: process.env.PLAYWRIGHT_CHANNEL || 'msedge' });
  try {
    const page = await browser.newPage({ viewport: { width: 1366, height: 768 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      localStorage.setItem('remotex-language', 'en');
      localStorage.setItem('remotex-appearance', 'light');
      localStorage.setItem('remotex-recent-devices', JSON.stringify([{ deviceId: '123456789', connectedAt: 1788822000000 }]));
      let password = 'TESTONLYABCDEFGH';
      let rotations = 0;
      let agent = { remoteAccessEnabled: true, serverUrl: 'https://39.96.68.170:7443', deviceName: 'STUDIO-WORKSTATION', caCertificatePath: 'test-ca.pem', allowInput: false, allowClipboard: false, allowFileUpload: false, allowFileDownload: false, fileRoots: '', unattendedAccess: true, unattendedSecret: '', secretConfigured: true, startWithWindows: false, videoQuality: 'auto' };
      const config = { controlServerUrl: agent.serverUrl, relayAddress: '39.96.68.170:7443', relayServerName: 'relay.39-96-68-170.sslip.io', caCertificatePath: 'test-ca.pem', stunAddress: '', configured: true };
      window.__testCalls = [];
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => 1,
        unregisterCallback: () => {},
        invoke: async (cmd, args) => {
          window.__testCalls.push(cmd);
          if (cmd === 'load_agent_settings') return { ...agent };
          if (cmd === 'save_agent_settings') { agent = { ...args.settings }; return; }
          if (cmd === 'load_server_config') return { config, needsSetup: false, migrationConflict: false, candidates: [] };
          if (cmd === 'agent_status' || cmd === 'start_agent' || cmd === 'stop_agent') return { running: agent.remoteAccessEnabled, state: agent.remoteAccessEnabled ? 'online' : 'offline', deviceId: '328491762', sessionId: window.__incomingSession || null, processId: 4321, startWithWindows: false };
          if (cmd === 'reveal_access_password') { if (window.__deferReveal) await new Promise(resolve => { window.__finishReveal = resolve; }); return password; }
          if (cmd === 'update_access_password') { password = args.password ?? 'TESTROTATION' + (++rotations).toString().padStart(4, '0'); agent.secretConfigured = true; return; }
          if (cmd.startsWith('check_')) return { ok: true, endpoint: config.controlServerUrl, message: 'Ready' };
          if (cmd === 'plugin:event|listen') return 1;
          return null;
        },
      };
    });
    await page.goto(process.env.REMOTEX_UI_URL || 'http://127.0.0.1:5173');
    await page.getByRole('heading', { name: 'Allow a connection' }).waitFor();
    await page.getByRole('button', { name: 'Show password', exact: true }).click();
    assert.equal(await page.getByLabel('Local connection password').inputValue(), 'TESTONLYABCDEFGH');
    await page.getByRole('button', { name: 'Hide password', exact: true }).click();
    for (let i = 0; i < 3; i++) await page.getByRole('button', { name: 'Generate another' }).click();
    await page.getByRole('button', { name: 'Show password', exact: true }).click();
    assert.equal(await page.getByLabel('Local connection password').inputValue(), 'TESTROTATION0003');
    await page.getByRole('button', { name: 'Hide password', exact: true }).click();
    await page.getByRole('button', { name: 'Custom password', exact: true }).click();
    await page.getByLabel('New password', { exact: true }).fill('custom-test-password');
    await page.getByLabel('Confirm password', { exact: true }).fill('wrong');
    assert(await page.getByRole('button', { name: 'Save password', exact: true }).isDisabled());
    await page.getByLabel('Confirm password', { exact: true }).fill('custom-test-password');
    await page.getByRole('button', { name: 'Save password', exact: true }).click();
    await page.getByRole('button', { name: 'Show password', exact: true }).click();
    assert.equal(await page.getByLabel('Local connection password').inputValue(), 'custom-test-password');
    await page.evaluate(() => window.dispatchEvent(new Event('blur')));
    assert.equal(await page.getByLabel('Local connection password').getAttribute('type'), 'password');
    await page.evaluate(() => { window.__deferReveal = true; });
    await page.getByRole('button', { name: 'Show password', exact: true }).click();
    await page.waitForFunction(() => typeof window.__finishReveal === 'function');
    await page.evaluate(() => { window.dispatchEvent(new Event('blur')); window.__finishReveal(); window.__deferReveal = false; });
    await page.waitForFunction(() => !document.querySelector('[aria-label="Show password"]').disabled);
    assert.equal(await page.getByLabel('Local connection password').getAttribute('type'), 'password');
    await page.getByRole('button', { name: 'Custom password', exact: true }).click();
    await page.getByLabel('New password', { exact: true }).fill('active-session-test');
    await page.getByLabel('Confirm password', { exact: true }).fill('active-session-test');
    await page.evaluate(() => { window.__incomingSession = 'incoming-test'; });
    await page.waitForFunction(() => [...document.querySelectorAll('button')].find(button => button.textContent === 'Save password')?.disabled);
    const changesBefore = await page.evaluate(() => window.__testCalls.filter(cmd => cmd === 'update_access_password').length);
    await page.getByLabel('Confirm password', { exact: true }).press('Enter');
    assert.equal(await page.evaluate(() => window.__testCalls.filter(cmd => cmd === 'update_access_password').length), changesBefore);
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    await page.evaluate(() => { window.__incomingSession = null; });
    await page.waitForFunction(() => ![...document.querySelectorAll('button')].find(button => button.textContent.includes('Generate another'))?.disabled);
    await page.getByLabel('Remote device ID', { exact: true }).fill('328491762');
    assert(await page.getByRole('button', { name: 'Connect to device', exact: true }).isDisabled());
    await page.getByLabel('Remote device ID', { exact: true }).fill('123456789');
    await page.locator('#remote-access-password').fill('not-saved-password');
    await page.getByRole('button', { name: 'Show or hide remote password' }).click();
    assert.equal(await page.locator('#remote-access-password').getAttribute('type'), 'text');
    await page.evaluate(() => window.dispatchEvent(new Event('blur')));
    assert.equal(await page.locator('#remote-access-password').getAttribute('type'), 'password');
    await page.getByLabel('Remote device ID', { exact: true }).fill('987654321');
    assert.equal(await page.locator('#remote-access-password').inputValue(), '');
    assert(!(await page.evaluate(() => JSON.stringify(localStorage))).includes('custom-test-password'));
    await page.getByRole('button', { name: 'Devices', exact: true }).click();
    await page.getByLabel('Search recent devices').fill('000');
    await page.getByText('No matching devices.').waitFor();
    await page.getByLabel('Search recent devices').fill('');
    await page.getByRole('button', { name: 'Remove recent device', exact: true }).click();
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    assert.equal(await page.getByRole('button', { name: 'Remove recent device', exact: true }).count(), 1);
    for (const language of ['en', 'zh-CN']) {
      await page.getByRole('button', { name: /^(Settings|设置)$/ }).click();
      await page.getByLabel(/^(Interface language|界面语言)$/).selectOption(language);
      for (const theme of ['dark', 'light']) {
        // Appearance test uses the same semantic CSS tokens; native chrome is separately maintained by useAppearance.
        await page.evaluate(theme => { document.documentElement.dataset.theme = theme; }, theme);
        for (const [width, height] of [[1280, 720], [1366, 768], [1920, 1080]]) {
          await page.setViewportSize({ width, height });
          for (const [english, chinese] of [['Home', '主页'], ['Devices', '设备'], ['Files', '文件'], ['Settings', '设置'], ['About', '关于']]) {
            await page.getByRole('button', { name: language === 'en' ? english : chinese, exact: true }).first().click();
            await page.mouse.move(0, 0);
            const overflow = await page.locator('.page-content').evaluate(el => el.scrollWidth > el.clientWidth + 1);
            assert(!overflow, 'Horizontal overflow: ' + [language, theme, width, english].join('/'));
            if (width === 1366 || english === 'Home') await page.screenshot({ path: path.join(output, [language, theme, width, english].join('-') + '.png') });
          }
        }
      }
    }
    assert.deepEqual(errors, []);
    console.log('PASS: password reveal/rotate/custom validation/hide, secret storage, self-ID, target switch, recent search/remove/undo; 60 page/locale/theme/size checks.');
    console.log('Screenshots: ' + output);
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
