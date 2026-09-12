//! Canonical desktop contract. Generated TypeScript contains domain data only.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Send,
    Receive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TransferMode {
    OneWay,
    Reliable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Preparing,
    Listening,
    Acquiring,
    Receiving,
    Transmitting,
    WaitingFeedback,
    Retransmitting,
    Verifying,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FileInfo {
    pub name: String,
    pub bytes: u32,
    pub data_packets: u32,
    pub total_frames: u32,
    pub duration_seconds: f64,
    pub sha256: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct Progress {
    pub transfer_id: Option<String>,
    pub received_packets: u32,
    pub total_packets: Option<u32>,
    pub bytes_received: Option<u32>,
    pub total_bytes: Option<u32>,
    pub valid_frames: u32,
    pub rejected_frames: u32,
    pub crc_failures: u32,
    pub duplicate_frames: u32,
    pub retransmissions: u32,
    pub queued_packets: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SignalMetrics {
    pub samples: f64,
    pub rms: f64,
    pub peak: f32,
    pub clipped_samples: u32,
    pub gaps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FrameMetrics {
    pub start_sample: f64,
    pub acquisition_quality: f64,
    pub tone_concentration: f64,
    pub carrier_powers: [f64; 2],
    pub decision_ratio: f64,
    pub samples_per_symbol: f64,
    pub clock_ppm: Option<f64>,
    pub acquisition_window_samples: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidArgument,
    Busy,
    DeviceUnavailable,
    UnsupportedAudio,
    PermissionDenied,
    AudioInterrupted,
    NoFrames,
    MissingMetadata,
    MissingPackets,
    CrcFailure,
    Integrity,
    Timeout,
    Destination,
    DiagnosticWrite,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Failure {
    pub code: ErrorCode,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Completion {
    Received { file: FileInfo, output_path: String },
    Sent { file: FileInfo, peer_verified: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionStatus {
    Active { phase: Phase },
    Completed { result: Completion },
    Failed { error: Failure },
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    StateChanged {
        phase: Phase,
    },
    MetadataReceived {
        transfer_id: String,
        file: FileInfo,
    },
    Progress {
        progress: Progress,
    },
    Signal {
        metrics: SignalMetrics,
    },
    FrameDetected {
        metrics: FrameMetrics,
    },
    FrameAccepted {
        transfer_id: String,
        flags: u8,
        sequence: u16,
        bytes: u32,
    },
    FrameRejected {
        code: ErrorCode,
        detail: String,
    },
    AudioGap {
        gaps: u32,
        sample: f64,
    },
    FrameQueued {
        transfer_id: String,
        flags: u8,
        sequence: u16,
        bytes: u32,
    },
    RetransmissionRequested,
    Warning {
        error: Failure,
    },
    Completed {
        result: Completion,
    },
    Failed {
        error: Failure,
    },
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SessionEvent {
    pub sequence: u32,
    pub elapsed_ms: u32,
    pub event: Event,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SessionSnapshot {
    pub id: String,
    pub direction: Direction,
    pub mode: TransferMode,
    pub status: SessionStatus,
    pub elapsed_ms: u32,
    pub progress: Progress,
    pub file: Option<FileInfo>,
    pub signal: Option<SignalMetrics>,
    pub frame: Option<FrameMetrics>,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub input_channel: u32,
    pub destination: Option<String>,
    pub capture_path: Option<String>,
    pub events_path: Option<String>,
    pub last_sequence: u32,
    pub warnings: Vec<Failure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SessionUpdate {
    pub snapshot: Option<SessionSnapshot>,
    pub events: Vec<SessionEvent>,
    pub events_truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct DeviceSelection {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub input_channel: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub is_input: bool,
    pub is_default: bool,
    pub compatible: bool,
    pub channels: Option<u32>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SessionRequest {
    pub direction: Direction,
    pub mode: TransferMode,
    pub path: String,
    pub devices: DeviceSelection,
    pub capture_path: Option<String>,
    pub events_path: Option<String>,
    pub max_seconds: u32,
    pub idle_timeout_seconds: u32,
    pub overwrite: bool,
    pub expected_sha256: Option<String>,
}

pub type Observer = std::sync::Arc<dyn Fn(Event) + Send + Sync>;

pub fn file_info(metadata: &tonequill_core::transfer::Metadata) -> FileInfo {
    let plan = tonequill_core::transfer::waveform::TransmissionPlan::new(metadata)
        .expect("validated metadata");
    FileInfo {
        name: metadata.filename.clone(),
        bytes: metadata.file_size as u32,
        data_packets: metadata.data_packets,
        total_frames: plan.total_frames,
        duration_seconds: plan.duration_seconds(),
        sha256: metadata.sha256.iter().map(|b| format!("{b:02x}")).collect(),
    }
}
