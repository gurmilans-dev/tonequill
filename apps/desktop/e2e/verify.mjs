import { chromium } from 'playwright';
import AxeBuilder from '@axe-core/playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

// Deterministic UI verification. The app labels every fixture as development
// demo; this is deliberately not evidence of a physical acoustic transfer.
const output = resolve('../../target/phase4/ui');
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({
  viewport: { width: 1160, height: 820 },
  reducedMotion: 'reduce',
});
const page = await context.newPage();
const errors = [];
const accessibility = [];
page.on('pageerror', (error) => errors.push(error.message));
async function screenshot(name) {
  const audit = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21aa'])
    .analyze();
  accessibility.push({ name, violations: audit.violations });
  await writeFile(resolve(output, 'accessibility.json'), JSON.stringify(accessibility, null, 2));
  assert.deepEqual(
    audit.violations.map((v) => ({ id: v.id, nodes: v.nodes.map((n) => n.target) })),
    [],
    `${name}: accessibility violations`,
  );
  await page.screenshot({ path: resolve(output, `${name}.png`), fullPage: true });
  assert.ok(
    await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
    `${name}: horizontal overflow`,
  );
}
try {
  await page.goto('http://127.0.0.1:1420/?demo');
  await page.getByRole('button', { name: /Receive a file/ }).waitFor();
  await screenshot('home-light');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('combobox', { name: 'Appearance' }).selectOption('dark');
  await screenshot('settings-dark');
  await page.getByRole('button', { name: 'Transfer', exact: true }).click();
  await screenshot('home-dark');
  await page.getByRole('button', { name: /Receive a file/ }).click();
  await screenshot('receive-setup');
  await page.getByRole('button', { name: /Choose where to save/ }).click();
  await page.getByRole('button', { name: 'Start listening' }).click();
  await page.getByRole('heading', { name: 'Listening', exact: true }).waitFor();
  assert.equal(await page.getByRole('progressbar').getAttribute('aria-valuenow'), null);
  await screenshot('listening-unknown-total');
  await page.getByRole('heading', { name: 'File received.' }).waitFor();
  await screenshot('receive-verified');
  await page.getByRole('button', { name: 'View diagnostics' }).click();
  await screenshot('diagnostics');
  await page.setViewportSize({ width: 760, height: 620 });
  await screenshot('diagnostics-small');
  await page.getByRole('button', { name: 'Transfer', exact: true }).click();
  await screenshot('verified-small');
  await page.getByRole('button', { name: 'Receive another' }).click();
  await page.getByRole('button', { name: /Choose where to save/ }).click();
  await page.getByRole('button', { name: 'Start listening' }).click();
  await page.getByRole('button', { name: 'Cancel session' }).click();
  await page.getByRole('heading', { name: 'Session cancelled.' }).waitFor();
  await screenshot('cancelled-small');
  await page.goto('http://127.0.0.1:1420/?demo=failure');
  await page.getByRole('button', { name: /Receive a file/ }).click();
  await page.getByRole('button', { name: /Choose where to save/ }).click();
  await page.getByRole('button', { name: 'Start listening' }).click();
  await page.getByRole('heading', { name: 'Transfer incomplete' }).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Show in folder' }).count(), 0);
  await screenshot('incomplete-small');
  await page.goto('http://127.0.0.1:1420/?demo');
  await page.setViewportSize({ width: 1160, height: 820 });
  await page.getByRole('button', { name: /Send a file/ }).click();
  await page.getByRole('button', { name: /Drop a file here/ }).click();
  await screenshot('send-plan');
  await page.getByRole('button', { name: 'Start transmission' }).click();
  await page.getByRole('heading', { name: 'Playback complete.' }).waitFor();
  await screenshot('send-complete');
  assert.equal(await page.getByText('Verified', { exact: true }).count(), 0);
  assert.deepEqual(errors, []);
  await writeFile(
    resolve(output, 'result.json'),
    JSON.stringify(
      {
        status: 'PASS',
        browser: await browser.version(),
        screenshots: 13,
        scenarios: [
          'home light/dark',
          'file planning',
          'unknown totals',
          'verified receive',
          'diagnostics',
          'small layouts',
          'cancellation',
          'incomplete transfer',
          'unconfirmed one-way send',
        ],
        physicalValidation: 'NOT YET TESTED',
        pageErrors: errors,
      },
      null,
      2,
    ),
  );
  console.log('UI verification PASS; screenshots in target/phase4/ui');
} finally {
  await browser.close();
}
