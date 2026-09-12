// Loaded only by a development build with ?demo. Never a production fallback.
import type { Bridge } from './bridge';
import type { FileInfo, SessionRequest, SessionSnapshot } from './domain/generated';
export const demoFile: FileInfo = {
  name: 'hello.txt',
  bytes: 17,
  data_packets: 1,
  total_frames: 2,
  duration_seconds: 10.52,
  sha256: 'fc7ced9fbe4ae5c9c9968ee1a65246833ce2e0c0f6bed65a4649b1bcd70438ae',
};
export function demoSnapshot(request: SessionRequest, id = 'demo-1'): SessionSnapshot {
  return {
    id,
    direction: request.direction,
    mode: request.mode,
    status: { kind: 'active', phase: 'listening' },
    elapsed_ms: 0,
    progress: {
      transfer_id: null,
      received_packets: 0,
      total_packets: null,
      bytes_received: 0,
      total_bytes: null,
      valid_frames: 0,
      rejected_frames: 0,
      crc_failures: 0,
      duplicate_frames: 0,
      retransmissions: 0,
      queued_packets: 0,
    },
    file: null,
    signal: null,
    frame: null,
    input_device: request.devices.input_device,
    output_device: request.devices.output_device,
    input_channel: request.devices.input_channel,
    destination: request.direction === 'receive' ? request.path : null,
    capture_path: request.capture_path,
    events_path: request.events_path,
    last_sequence: 1,
    warnings: [],
  };
}
export function createDemo(): Bridge {
  let current: SessionSnapshot | null = null;
  let startTime = 0;
  let count = 0;
  const scenario = new URLSearchParams(location.search).get('demo');
  return {
    demo: true,
    devices: async () => [
      {
        id: 'demo-mic',
        name: 'Built-in Microphone',
        is_input: true,
        is_default: true,
        compatible: true,
        channels: 2,
        detail: null,
      },
      {
        id: 'demo-speaker',
        name: 'USB Speakers',
        is_input: false,
        is_default: true,
        compatible: true,
        channels: 2,
        detail: null,
      },
    ],
    inspect: async () => ({ ...demoFile }),
    start: async (request) => {
      current = demoSnapshot(request, `demo-${++count}`);
      startTime = Date.now();
      return current.id;
    },
    cancel: async () => {
      await new Promise((resolve) => setTimeout(resolve, 150));
      if (current) {
        current.status = { kind: 'cancelled' };
        current.last_sequence++;
      }
    },
    poll: async (after) => {
      if (!current) return { snapshot: null, events: [], events_truncated: false };
      if (current.status.kind === 'active') {
        current.elapsed_ms = Date.now() - startTime;
        const stage = Math.floor(current.elapsed_ms / 2000);
        current.signal = {
          samples: Math.floor(current.elapsed_ms * 48),
          rms: 0.034,
          peak: 0.16,
          clipped_samples: 0,
          gaps: 0,
        };
        if (stage >= 1 && scenario !== 'unknown') {
          current.file = demoFile;
          current.progress.transfer_id = 'DEMO0001';
          current.progress.total_bytes = demoFile.bytes;
          current.progress.total_packets = 1;
          current.progress.valid_frames = 1;
          current.status = {
            kind: 'active',
            phase: current.direction === 'receive' ? 'receiving' : 'transmitting',
          };
          current.frame = {
            start_sample: 9600,
            acquisition_quality: 0.92,
            tone_concentration: 0.88,
            carrier_powers: [0.0081, 0.0073],
            decision_ratio: 1.1,
            samples_per_symbol: 480.03,
            clock_ppm: 62.5,
            acquisition_window_samples: 85440,
          };
        }
        if (stage >= 3 && scenario !== 'unknown') {
          if (scenario === 'failure') {
            current.progress.rejected_frames = 1;
            current.progress.crc_failures = 1;
            current.status = {
              kind: 'failed',
              error: {
                code: 'missing_packets',
                detail: 'Demonstration: 0 of 1 data packets received before the session deadline.',
              },
            };
          } else {
            current.progress.received_packets =
              current.direction === 'receive' || current.mode === 'reliable' ? 1 : 0;
            current.progress.queued_packets = current.direction === 'send' ? 1 : 0;
            current.progress.bytes_received = current.direction === 'receive' ? 17 : 0;
            current.progress.valid_frames = current.direction === 'receive' ? 2 : 0;
            current.status = {
              kind: 'completed',
              result:
                current.direction === 'receive'
                  ? {
                      kind: 'received',
                      file: demoFile,
                      output_path: current.destination ?? 'C:\\Demo\\received.txt',
                    }
                  : { kind: 'sent', file: demoFile, peer_verified: current.mode === 'reliable' },
            };
          }
        }
        current.last_sequence++;
      }
      return {
        snapshot: structuredClone(current),
        events:
          current.last_sequence > after
            ? [
                {
                  sequence: current.last_sequence,
                  elapsed_ms: current.elapsed_ms,
                  event:
                    current.status.kind === 'active'
                      ? { type: 'state_changed', phase: current.status.phase }
                      : current.status.kind === 'completed'
                        ? { type: 'completed', result: current.status.result }
                        : current.status.kind === 'failed'
                          ? { type: 'failed', error: current.status.error }
                          : { type: 'cancelled' },
                },
              ]
            : [],
        events_truncated: false,
      };
    },
    pickFile: async () => 'C:\\Demo\\hello.txt',
    pickDestination: async (suggested) => `C:\\Demo\\${suggested}`,
    reveal: async () => {},
    quit: async () => {},
    onDrop: async () => () => {},
  };
}
