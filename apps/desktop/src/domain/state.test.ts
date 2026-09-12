import { describe, expect, it } from 'vitest';
import { demoFile, demoSnapshot } from '../demo';
import { initialSession, percent, sessionReducer, signalLabel } from './state';
import { request } from '../test/fixtures';
describe('authoritative session state', () => {
  it('does not rerender an unchanged terminal snapshot during idle polling', () => {
    const s = demoSnapshot(request);
    s.status = { kind: 'cancelled' };
    const update = { snapshot: s, events: [], events_truncated: false };
    const state = sessionReducer(initialSession, { type: 'update', update });
    expect(sessionReducer(state, { type: 'update', update: structuredClone(update) })).toBe(state);
  });
  it('keeps total unknown until metadata arrives', () => {
    const s = demoSnapshot(request);
    expect(percent(s)).toBeNull();
    s.progress.total_packets = 2;
    s.progress.received_packets = 1;
    expect(percent(s)).toBe(50);
  });
  it('does not turn one-way playback into remote confirmation', () => {
    const s = demoSnapshot({ ...request, direction: 'send' });
    s.progress.total_packets = 2;
    s.progress.queued_packets = 1;
    expect(percent(s)).toBe(50);
    expect(s.progress.received_packets).toBe(0);
  });
  it('ignores stale IDs, old snapshots and repeated events', () => {
    let state = sessionReducer(initialSession, { type: 'starting' });
    state = sessionReducer(state, { type: 'started', id: 'new' });
    const old = demoSnapshot(request, 'old');
    expect(
      sessionReducer(state, {
        type: 'update',
        update: { snapshot: old, events: [], events_truncated: false },
      }),
    ).toBe(state);
    const next = demoSnapshot(request, 'new');
    next.last_sequence = 4;
    const update = {
      snapshot: next,
      events: [
        {
          sequence: 4,
          elapsed_ms: 100,
          event: { type: 'state_changed' as const, phase: 'listening' as const },
        },
      ],
      events_truncated: false,
    };
    state = sessionReducer(state, { type: 'update', update });
    state = sessionReducer(state, { type: 'update', update });
    expect(state.events).toHaveLength(1);
    expect(
      sessionReducer(state, {
        type: 'update',
        update: { ...update, snapshot: { ...next, last_sequence: 2 } },
      }),
    ).toBe(state);
  });
  it('waits for backend cleanup before marking cancelled', () => {
    const s = demoSnapshot(request);
    let state = sessionReducer(initialSession, {
      type: 'update',
      update: { snapshot: s, events: [], events_truncated: false },
    });
    state = sessionReducer(state, { type: 'cancelling' });
    expect(state.snapshot?.status.kind).toBe('active');
    expect(state.pending).toBe('cancel');
    s.status = { kind: 'cancelled' };
    state = sessionReducer(state, {
      type: 'update',
      update: { snapshot: s, events: [], events_truncated: false },
    });
    expect(state.pending).toBeNull();
  });
  it('retains verified completion if cancellation loses the commit race', () => {
    const s = demoSnapshot(request);
    let state = sessionReducer(initialSession, {
      type: 'update',
      update: { snapshot: s, events: [], events_truncated: false },
    });
    state = sessionReducer(state, { type: 'cancelling' });
    s.status = {
      kind: 'completed',
      result: { kind: 'received', file: demoFile, output_path: request.path },
    };
    state = sessionReducer(state, {
      type: 'update',
      update: { snapshot: s, events: [], events_truncated: false },
    });
    expect(state.snapshot?.status.kind).toBe('completed');
    expect(state.pending).toBeNull();
  });
  it('bounds logs and does not reopen dismissed sessions', () => {
    const s = demoSnapshot(request);
    s.last_sequence = 500;
    const events = Array.from({ length: 500 }, (_, i) => ({
      sequence: i + 1,
      elapsed_ms: i,
      event: {
        type: 'signal' as const,
        metrics: { samples: i, rms: 0, peak: 0, gaps: 0, clipped_samples: 0 },
      },
    }));
    let state = sessionReducer(initialSession, {
      type: 'update',
      update: { snapshot: s, events, events_truncated: true },
    });
    expect(state.events).toHaveLength(256);
    state = sessionReducer(state, { type: 'reset' });
    expect(
      sessionReducer(state, {
        type: 'update',
        update: { snapshot: s, events, events_truncated: false },
      }).snapshot,
    ).toBeNull();
  });
  it('uses measured signal levels without inventing quality', () => {
    const s = demoSnapshot(request);
    expect(signalLabel(s)).toBe('Awaiting audio');
    s.signal = { samples: 48000, rms: 0.002, peak: 0.03, gaps: 0, clipped_samples: 0 };
    expect(signalLabel(s)).toBe('Quiet');
    s.signal.clipped_samples = 1;
    expect(signalLabel(s)).toBe('Clipping detected');
  });
});
