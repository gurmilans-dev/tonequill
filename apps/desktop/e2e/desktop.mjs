import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import { mkdir, access, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';

// Windows production smoke test. Uses the real Tauri executable, IPC and CPAL.
// It briefly opens the microphone, stores no capture, and transmits no audio.
// The temporary CDP port/profile are provided only to this child process.
const root = resolve('../..');
const output = resolve(root, 'target/phase4/native');
await mkdir(output, { recursive: true });
const exe = process.env.TONEQUILL_DESKTOP_EXE
  ? resolve(process.env.TONEQUILL_DESKTOP_EXE)
  : resolve(root, 'target/release/tonequill-desktop.exe');
const app = spawn(exe, [], {
  windowsHide: true,
  stdio: 'ignore',
  env: {
    ...process.env,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=9224',
    WEBVIEW2_USER_DATA_FOLDER: resolve(output, `profile-${Date.now()}`),
  },
});
let browser;
let page;
let exitCode;
app.on('exit', (code) => {
  exitCode = code;
});
const errors = [];
const report = { status: 'RUNNING', checks: [], physicalValidation: 'IN PROGRESS — FAILING' };
async function until(predicate, timeout = 20000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (await predicate()) return;
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error('Desktop smoke test timed out');
}
try {
  await until(async () => {
    try {
      return (await fetch('http://127.0.0.1:9224/json/version')).ok;
    } catch {
      return false;
    }
  });
  browser = await chromium.connectOverCDP('http://127.0.0.1:9224');
  const context = browser.contexts()[0];
  await until(() => context.pages().length > 0);
  page = context.pages()[0];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.getByRole('button', { name: /Receive a file/ }).waitFor();
  assert.equal(await page.getByText(/DEVELOPMENT DEMO/).count(), 0);
  const invoke = (command, args = {}) =>
    page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), {
      command,
      args,
    });
  const devices = await invoke('audio_devices');
  report.devices = devices.map((d) => ({
    name: d.name,
    input: d.is_input,
    compatible: d.compatible,
  }));
  report.checks.push('Production frontend loads with restrictive CSP and real device discovery');
  await page.screenshot({ path: resolve(output, 'home.png'), fullPage: true });
  for (const [file, bytes] of [
    ['hello.txt', 17],
    ['physical-257.bin', 257],
  ]) {
    const info = await invoke('inspect_file', { path: resolve(root, 'test-vectors', file) });
    assert.equal(info.bytes, bytes);
    report.checks.push(
      `${file}: Rust inspection returns ${bytes} bytes, ${info.data_packets} data packets`,
    );
  }
  const input =
    devices.find((d) => d.is_input && d.compatible && d.is_default) ??
    devices.find((d) => d.is_input && d.compatible);
  const speaker =
    devices.find((d) => !d.is_input && d.compatible && d.is_default) ??
    devices.find((d) => !d.is_input && d.compatible);
  assert.ok(input && speaker, 'Real audio input/output required for this smoke test');
  const destination = resolve(output, `must-not-exist-${Date.now()}.bin`);
  const request = {
    direction: 'receive',
    mode: 'one_way',
    path: destination,
    devices: { input_device: input.id, output_device: speaker.id, input_channel: 0 },
    capture_path: null,
    events_path: null,
    max_seconds: 60,
    idle_timeout_seconds: 60,
    overwrite: false,
    expected_sha256: null,
  };
  const id = await invoke('start_session', { request });
  let snapshot;
  await until(async () => {
    snapshot = (await invoke('poll_session', { after: 0 })).snapshot;
    if (snapshot.status.kind === 'failed') throw new Error(JSON.stringify(snapshot.status));
    return snapshot.signal?.samples > 0;
  });
  assert.equal(snapshot.id, id);
  await page.getByRole('button', { name: 'Cancel session' }).waitFor();
  await page.screenshot({ path: resolve(output, 'listening.png'), fullPage: true });
  await page.getByRole('button', { name: 'Cancel session' }).click();
  await page.getByRole('heading', { name: 'Session cancelled.' }).waitFor();
  assert.equal((await invoke('poll_session', { after: 0 })).snapshot.status.kind, 'cancelled');
  await assert.rejects(access(destination));
  report.checks.push(
    `Real CPAL captured ${snapshot.signal.samples} samples; UI cancellation joined worker; no final file`,
  );
  await page.getByRole('button', { name: 'Receive another' }).click();
  await invoke('start_session', {
    request: {
      ...request,
      devices: { ...request.devices, input_device: 'intentionally-removed-device-for-smoke-test' },
    },
  });
  await page.getByRole('heading', { name: 'Audio device unavailable' }).waitFor();
  await page.screenshot({ path: resolve(output, 'device-error.png'), fullPage: true });
  report.checks.push('Removed device produces typed failure in actual production UI');
  await page.getByRole('button', { name: 'Receive another' }).click();
  await invoke('start_session', { request });
  await until(async () => {
    const s = (await invoke('poll_session', { after: 0 })).snapshot;
    return s.signal?.samples > 0;
  });
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Quit Tonequill' }).click();
  await until(() => exitCode !== undefined, 15000);
  assert.equal(exitCode, 0);
  await assert.rejects(access(destination));
  report.checks.push(
    'Quit while receiving terminates worker and application cleanly with no final file',
  );
  assert.deepEqual(errors, []);
  report.status = 'PASS';
  console.log('Native production smoke PASS');
} catch (error) {
  report.status = 'FAIL';
  report.error = String(error);
  if (page) {
    try {
      await page.screenshot({ path: resolve(output, 'failure.png'), fullPage: true });
    } catch {
      /* Failure capture is unavailable after the webview has closed. */
    }
  }
  throw error;
} finally {
  await writeFile(resolve(output, 'result.json'), JSON.stringify(report, null, 2));
  if (exitCode === undefined) {
    try {
      if (page) await page.evaluate(() => window.__TAURI_INTERNALS__.invoke('plugin:window|close'));
    } catch {
      /* The native close event may already have stopped IPC. */
    }
    await new Promise((r) => setTimeout(r, 2000));
    if (exitCode === undefined) app.kill();
  }
  if (browser) {
    try {
      await browser.close();
    } catch {
      /* Closing a disconnected CDP client needs no further action. */
    }
  }
}
