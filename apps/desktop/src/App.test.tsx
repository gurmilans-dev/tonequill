import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { App } from './App';
import { createDemo, demoFile, demoSnapshot } from './demo';
import { request } from './test/fixtures';
import { SessionPanel } from './components/SessionPanel';
import {
  addRecent,
  defaultPreferences,
  readPreferences,
  readRecent,
  savePreferences,
  saveRecent,
} from './storage';

describe('desktop flows', () => {
  it('starts one-way receive with an exact chosen path and diagnostics off', async () => {
    const bridge = createDemo();
    bridge.demo = false;
    const start = vi.spyOn(bridge, 'start');
    const user = userEvent.setup();
    render(<App bridge={bridge} />);
    await user.click(screen.getByRole('button', { name: /Receive a file/ }));
    expect(screen.getByRole('button', { name: 'Start listening' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: /Choose where to save/ }));
    await user.click(screen.getByRole('button', { name: 'Start listening' }));
    await waitFor(() => expect(start).toHaveBeenCalledOnce());
    expect(start.mock.calls[0]?.[0]).toMatchObject({
      direction: 'receive',
      mode: 'one_way',
      overwrite: false,
      capture_path: null,
      events_path: null,
    });
    expect(start.mock.calls[0]?.[0].path).toContain('C:\\Demo\\received-');
    await screen.findByRole('heading', { name: 'Listening' });
  });
  it('inspects a source and sends its expected hash to Rust', async () => {
    const bridge = createDemo();
    const start = vi.spyOn(bridge, 'start');
    const user = userEvent.setup();
    render(<App bridge={bridge} />);
    await user.click(screen.getByRole('button', { name: /Send a file/ }));
    await user.click(screen.getByRole('button', { name: /Drop a file here/ }));
    expect(await screen.findByText('10.5 s')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Start transmission' }));
    await waitFor(() =>
      expect(start).toHaveBeenCalledWith(
        expect.objectContaining({ expected_sha256: demoFile.sha256, direction: 'send' }),
      ),
    );
  });
  it('shows useful failure information without claiming a file exists', () => {
    const s = demoSnapshot(request);
    s.status = {
      kind: 'failed',
      error: { code: 'missing_packets', detail: 'Missing data packet 1' },
    };
    render(
      <SessionPanel
        s={s}
        cancelling={false}
        cancel={() => {}}
        again={() => {}}
        diagnostics={() => {}}
        reveal={() => {}}
      />,
    );
    expect(screen.getByRole('heading', { name: 'Transfer incomplete' })).toBeInTheDocument();
    expect(screen.getByText(/File written: No/)).toBeInTheDocument();
    expect(screen.getByRole('progressbar').firstElementChild).toHaveStyle({ width: '0%' });
    expect(screen.queryByRole('button', { name: 'Show in folder' })).not.toBeInTheDocument();
  });
  it('distinguishes one-way send completion from a verified received file', () => {
    const s = demoSnapshot({ ...request, direction: 'send' });
    s.status = {
      kind: 'completed',
      result: { kind: 'sent', file: demoFile, peer_verified: false },
    };
    render(
      <SessionPanel
        s={s}
        cancelling={false}
        cancel={() => {}}
        again={() => {}}
        diagnostics={() => {}}
        reveal={() => {}}
      />,
    );
    expect(screen.getByRole('heading', { name: 'Playback complete.' })).toBeInTheDocument();
    expect(screen.getByText(/cannot confirm reception/)).toBeInTheDocument();
    expect(screen.queryByText('Verified')).not.toBeInTheDocument();
  });
  it('exposes technical device failures and allows a refreshed list', async () => {
    const bridge = createDemo();
    const devices = vi
      .spyOn(bridge, 'devices')
      .mockRejectedValueOnce({ code: 'permission_denied', detail: 'Permission revoked' });
    const user = userEvent.setup();
    render(<App bridge={bridge} />);
    expect(await screen.findByRole('alert')).toHaveTextContent('Microphone access denied');
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Refresh devices' }));
    await waitFor(() => expect(devices).toHaveBeenCalledTimes(2));
  });
});
describe('bounded local preferences and history', () => {
  it('round-trips valid preferences and bounds hostile channels and limits', () => {
    savePreferences(defaultPreferences);
    expect(readPreferences()).toEqual(defaultPreferences);
    localStorage.setItem(
      'tonequill.preferences.v1',
      JSON.stringify({ maxSeconds: 999999, devices: { input_channel: -1 } }),
    );
    expect(readPreferences()).toEqual(defaultPreferences);
  });
  it('records verified completions once and bounds history to 30', () => {
    let rows: import('./storage').Recent[] = [];
    for (let i = 0; i < 40; i++) {
      const s = demoSnapshot(request, String(i));
      s.status = {
        kind: 'completed',
        result: { kind: 'received', file: demoFile, output_path: request.path },
      };
      rows = addRecent(rows, s);
      rows = addRecent(rows, s);
    }
    expect(rows).toHaveLength(30);
    saveRecent(rows);
    expect(readRecent()).toHaveLength(30);
  });
});
