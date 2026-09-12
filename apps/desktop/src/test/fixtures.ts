import type { SessionRequest } from '../domain/generated';
export const request: SessionRequest = {
  direction: 'receive',
  mode: 'one_way',
  path: 'C:\\Demo\\out.bin',
  devices: { input_device: null, output_device: null, input_channel: 0 },
  capture_path: null,
  events_path: null,
  max_seconds: 180,
  idle_timeout_seconds: 120,
  overwrite: false,
  expected_sha256: null,
};
