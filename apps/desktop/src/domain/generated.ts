// Generated from tonequill-live Rust types. Do not edit.
export type Direction = "send" | "receive";

export type TransferMode = "one_way" | "reliable";

export type Phase = "preparing" | "listening" | "acquiring" | "receiving" | "transmitting" | "waiting_feedback" | "retransmitting" | "verifying";

export type FileInfo = { name: string, bytes: number, data_packets: number, total_frames: number, duration_seconds: number, sha256: string, };

export type Progress = { transfer_id: string | null, received_packets: number, total_packets: number | null, bytes_received: number | null, total_bytes: number | null, valid_frames: number, rejected_frames: number, crc_failures: number, duplicate_frames: number, retransmissions: number, queued_packets: number, };

export type SignalMetrics = { samples: number, rms: number, peak: number, clipped_samples: number, gaps: number, };

export type FrameMetrics = { start_sample: number, acquisition_quality: number, tone_concentration: number, carrier_powers: [number, number], decision_ratio: number, samples_per_symbol: number, clock_ppm: number | null, acquisition_window_samples: number, };

export type ErrorCode = "invalid_argument" | "busy" | "device_unavailable" | "unsupported_audio" | "permission_denied" | "audio_interrupted" | "no_frames" | "missing_metadata" | "missing_packets" | "crc_failure" | "integrity" | "timeout" | "destination" | "diagnostic_write" | "internal";

export type Failure = { code: ErrorCode, detail: string, };

export type Completion = { "kind": "received", file: FileInfo, output_path: string, } | { "kind": "sent", file: FileInfo, peer_verified: boolean, };

export type SessionStatus = { "kind": "active", phase: Phase, } | { "kind": "completed", result: Completion, } | { "kind": "failed", error: Failure, } | { "kind": "cancelled" };

export type Event = { "type": "state_changed", phase: Phase, } | { "type": "metadata_received", transfer_id: string, file: FileInfo, } | { "type": "progress", progress: Progress, } | { "type": "signal", metrics: SignalMetrics, } | { "type": "frame_detected", metrics: FrameMetrics, } | { "type": "frame_accepted", transfer_id: string, flags: number, sequence: number, bytes: number, } | { "type": "frame_rejected", code: ErrorCode, detail: string, } | { "type": "audio_gap", gaps: number, sample: number, } | { "type": "frame_queued", transfer_id: string, flags: number, sequence: number, bytes: number, } | { "type": "retransmission_requested" } | { "type": "warning", error: Failure, } | { "type": "completed", result: Completion, } | { "type": "failed", error: Failure, } | { "type": "cancelled" };

export type SessionEvent = { sequence: number, elapsed_ms: number, event: Event, };

export type SessionSnapshot = { id: string, direction: Direction, mode: TransferMode, status: SessionStatus, elapsed_ms: number, progress: Progress, file: FileInfo | null, signal: SignalMetrics | null, frame: FrameMetrics | null, input_device: string | null, output_device: string | null, input_channel: number, destination: string | null, capture_path: string | null, events_path: string | null, last_sequence: number, warnings: Array<Failure>, };

export type SessionUpdate = { snapshot: SessionSnapshot | null, events: Array<SessionEvent>, events_truncated: boolean, };

export type DeviceSelection = { input_device: string | null, output_device: string | null, input_channel: number, };

export type DeviceInfo = { id: string, name: string, is_input: boolean, is_default: boolean, compatible: boolean, channels: number | null, detail: string | null, };

export type SessionRequest = { direction: Direction, mode: TransferMode, path: string, devices: DeviceSelection, capture_path: string | null, events_path: string | null, max_seconds: number, idle_timeout_seconds: number, overwrite: boolean, expected_sha256: string | null, };
