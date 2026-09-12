import type { ErrorCode, Failure, SessionEvent, SessionSnapshot, SessionUpdate } from './generated';

export interface SessionState {
  snapshot: SessionSnapshot | null;
  events: SessionEvent[];
  pending: 'start' | 'cancel' | null;
  expectedId: string | null;
  dismissedId: string | null;
  error: Failure | null;
  truncated: boolean;
}
export const initialSession: SessionState = {
  snapshot: null,
  events: [],
  pending: null,
  expectedId: null,
  dismissedId: null,
  error: null,
  truncated: false,
};
export type Action =
  | { type: 'starting' }
  | { type: 'started'; id: string }
  | { type: 'update'; update: SessionUpdate }
  | { type: 'cancelling' }
  | { type: 'error'; error: Failure }
  | { type: 'reset' };
export function sessionReducer(state: SessionState, action: Action): SessionState {
  switch (action.type) {
    case 'starting':
      return {
        ...initialSession,
        pending: 'start',
        dismissedId: state.snapshot?.id ?? state.dismissedId,
      };
    case 'started':
      return { ...state, pending: null, expectedId: action.id };
    case 'cancelling':
      return { ...state, pending: 'cancel', error: null };
    case 'error':
      return { ...state, pending: null, error: action.error };
    case 'reset':
      return { ...initialSession, dismissedId: state.snapshot?.id ?? state.dismissedId };
    case 'update': {
      const next = action.update.snapshot;
      if (
        !next ||
        state.pending === 'start' ||
        next.id === state.dismissedId ||
        (state.expectedId && next.id !== state.expectedId)
      )
        return state;
      const previous = state.snapshot;
      if (
        previous?.id === next.id &&
        previous.last_sequence === next.last_sequence &&
        previous.status.kind !== 'active' &&
        state.pending === null
      )
        return state;
      if (
        previous?.id === next.id &&
        (next.last_sequence < previous.last_sequence ||
          (previous.status.kind !== 'active' && next.status.kind === 'active'))
      )
        return state;
      const existing = previous?.id === next.id ? state.events : [];
      const lastSequence = existing.at(-1)?.sequence ?? 0;
      const events = [
        ...existing,
        ...action.update.events.filter((e) => e.sequence > lastSequence),
      ].slice(-256);
      return {
        ...state,
        snapshot: next,
        expectedId: next.id,
        events,
        truncated: state.truncated || action.update.events_truncated,
        pending: next.status.kind === 'active' ? state.pending : null,
      };
    }
  }
}
export function asFailure(value: unknown): Failure {
  if (
    typeof value === 'object' &&
    value !== null &&
    'code' in value &&
    'detail' in value &&
    typeof value.detail === 'string'
  )
    return value as Failure;
  return { code: 'internal', detail: value instanceof Error ? value.message : String(value) };
}
export const failureCopy: Record<ErrorCode, [string, string]> = {
  invalid_argument: ['Check your selection', 'Choose a valid file, destination and session limit.'],
  busy: ['Another session is active', 'Stop the current session before starting another.'],
  device_unavailable: [
    'Audio device unavailable',
    'Reconnect your microphone or speakers, then refresh devices in Settings.',
  ],
  unsupported_audio: [
    'Audio format unsupported',
    'Select a device that supports 48 kHz audio. Check the microphone channel in Settings.',
  ],
  permission_denied: [
    'Microphone access denied',
    'Allow desktop microphone access in Windows Settings, then retry.',
  ],
  audio_interrupted: [
    'Audio was interrupted',
    'Check the device connection and close applications using the audio device. Replay the whole transmission.',
  ],
  no_frames: [
    'No acoustic frames detected',
    'Replay the entire Tonequill WAV after listening starts. Check the microphone, phone volume and distance.',
  ],
  missing_metadata: [
    'File information is missing',
    'Some data arrived, but the opening metadata frame did not. Replay the whole WAV from the beginning.',
  ],
  missing_packets: [
    'Transfer incomplete',
    'Some data packets are missing. Move the devices closer and replay the whole WAV in a new session.',
  ],
  crc_failure: [
    'Audio frames could not be verified',
    'The signal arrived with errors. Move closer and check clipping before replaying.',
  ],
  integrity: [
    'File verification failed',
    'The received data did not pass integrity checks. Start a new session and replay the entire transmission.',
  ],
  timeout: [
    'Session time limit reached',
    'Choose a longer session limit if needed, then start again.',
  ],
  destination: [
    'Cannot save this file',
    'Choose an existing writable folder and a new file name, or explicitly allow replacement.',
  ],
  diagnostic_write: [
    'Cannot save diagnostics',
    'Choose new paths for the WAV and event log, or turn recording off.',
  ],
  internal: [
    'Session could not continue',
    'Check the details below, refresh your devices and try again.',
  ],
};
export function percent(snapshot: SessionSnapshot): number | null {
  const p = snapshot.progress;
  if (p.total_packets === null) return null;
  if (p.total_packets === 0) return snapshot.status.kind === 'completed' ? 100 : 0;
  const count =
    snapshot.direction === 'send' && snapshot.mode === 'one_way'
      ? p.queued_packets
      : p.received_packets;
  return Math.min(100, (count / p.total_packets) * 100);
}
export function signalLabel(snapshot: SessionSnapshot | null): string {
  const s = snapshot?.signal;
  if (!s) return 'Awaiting audio';
  if (s.clipped_samples > 0) return 'Clipping detected';
  if (s.peak >= 0.9) return 'Loud';
  return s.rms >= 0.003 ? 'Signal present' : 'Quiet';
}
export const phaseLabels = {
  preparing: 'Preparing',
  listening: 'Listening',
  acquiring: 'Acquiring a frame',
  receiving: 'Receiving',
  transmitting: 'Transmitting',
  waiting_feedback: 'Waiting for feedback',
  retransmitting: 'Retransmitting',
  verifying: 'Verifying',
};
export function duration(ms: number): string {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)
    .toString()
    .padStart(2, '0')}:${(s % 60).toString().padStart(2, '0')}`;
}
export function bytes(value: number): string {
  return value < 1024
    ? `${value} bytes`
    : value < 1024 ** 2
      ? `${(value / 1024).toFixed(1)} KB`
      : `${(value / 1024 ** 2).toFixed(1)} MB`;
}
export function basename(path: string): string {
  return path.split(/[\\/]/).at(-1) ?? path;
}
export function eventLabel(item: SessionEvent): string {
  const e = item.event;
  switch (e.type) {
    case 'state_changed':
      return phaseLabels[e.phase];
    case 'metadata_received':
      return `File metadata · ${e.file.name} · ${bytes(e.file.bytes)}`;
    case 'progress':
      return `Data packets · ${e.progress.received_packets} / ${e.progress.total_packets ?? 'unknown'}`;
    case 'signal':
      return `Audio · RMS ${e.metrics.rms.toFixed(5)} · peak ${e.metrics.peak.toFixed(5)}`;
    case 'frame_detected':
      return `Frame candidate · sample ${e.metrics.start_sample}`;
    case 'frame_accepted':
      return e.flags === 1
        ? `Metadata frame accepted · CRC valid`
        : e.flags === 2
          ? `Data packet ${e.sequence} accepted · CRC valid`
          : `Control frame accepted · CRC valid`;
    case 'frame_rejected':
      return `Frame rejected · ${e.detail}`;
    case 'audio_gap':
      return `Audio discontinuity · ${e.gaps} total`;
    case 'frame_queued':
      return `Frame queued for playback · sequence ${e.sequence}`;
    case 'retransmission_requested':
      return 'Feedback timed out · requesting receipt bitmap';
    case 'warning':
      return `Warning · ${e.error.detail}`;
    case 'completed':
      return e.result.kind === 'received'
        ? 'File saved · SHA-256 verified'
        : e.result.peer_verified
          ? 'Receiver confirmed verified file'
          : 'Playback finished · receiver confirmation unavailable';
    case 'failed':
      return `Failed · ${e.error.detail}`;
    case 'cancelled':
      return 'Cancelled · audio devices released';
  }
}
