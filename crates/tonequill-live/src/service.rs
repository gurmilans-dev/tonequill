//! Owned application sessions. No UI runtime, process-wide signal handler or DSP
//! implementation lives here. CPAL is created and dropped on the session worker.
use crate::{
    audio::{Audio, AudioOptions, AudioPort, InputEvent},
    errors::{AppError, failure},
    events::*,
    runtime::{
        self,
        capture::InputDiagnostics,
        link::{EventLog, Link},
    },
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::VecDeque,
    fs::File,
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tonequill_core::transfer::{
    Fragmenter, MAX_FILE_SIZE, VerifiedFile,
    reliable::{ReceiveSession, SendSession, Timing},
};

const EVENT_LIMIT: usize = 256;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn inspect_file(path: &Path) -> Result<FileInfo> {
    let (name, bytes) = read_input(path)?;
    Ok(file_info(Fragmenter::new(1, &name, &bytes)?.metadata()))
}

fn read_input(path: &Path) -> Result<(String, Vec<u8>)> {
    let file = File::open(path).context(AppError::new(
        ErrorCode::InvalidArgument,
        "Cannot read the selected file",
    ))?;
    ensure!(
        file.metadata()?.is_file(),
        AppError::new(ErrorCode::InvalidArgument, "Select a regular file")
    );
    ensure!(
        file.metadata()?.len() <= MAX_FILE_SIZE,
        AppError::new(
            ErrorCode::InvalidArgument,
            "File exceeds the 16 MiB transfer limit"
        )
    );
    let mut bytes = Vec::new();
    file.take(MAX_FILE_SIZE + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_SIZE,
        AppError::new(
            ErrorCode::InvalidArgument,
            "File grew beyond the transfer limit"
        )
    );
    let name = path
        .file_name()
        .context("File name is missing")?
        .to_string_lossy()
        .into_owned();
    Ok((name, bytes))
}

pub fn validate_request(request: &SessionRequest) -> Result<()> {
    ensure!(
        !request.path.trim().is_empty(),
        AppError::new(ErrorCode::InvalidArgument, "Choose an exact file path")
    );
    ensure!(
        (60..=3600).contains(&request.max_seconds),
        AppError::new(
            ErrorCode::InvalidArgument,
            "Session limit must be 60–3600 seconds"
        )
    );
    ensure!(
        (60..=request.max_seconds).contains(&request.idle_timeout_seconds),
        AppError::new(
            ErrorCode::InvalidArgument,
            "Idle timeout must be between 60 seconds and the session limit"
        )
    );
    ensure!(
        request.devices.input_channel < 32,
        AppError::new(
            ErrorCode::InvalidArgument,
            "Microphone channel must be 0–31"
        )
    );
    for id in [
        &request.devices.input_device,
        &request.devices.output_device,
    ]
    .into_iter()
    .flatten()
    {
        ensure!(
            !id.is_empty() && id.len() <= 4096 && !id.contains('\0'),
            AppError::new(ErrorCode::InvalidArgument, "Invalid audio device selector")
        );
    }
    if request.direction == Direction::Receive {
        validate_destination(Path::new(&request.path), request.overwrite)?;
    } else {
        ensure!(
            request.capture_path.is_none(),
            AppError::new(
                ErrorCode::InvalidArgument,
                "Microphone WAV capture is available for receive sessions"
            )
        );
    }
    runtime::validate_receive_paths(
        Path::new(&request.path),
        request.events_path.as_deref().map(Path::new),
        request.capture_path.as_deref().map(Path::new),
    )
    .context(AppError::new(
        ErrorCode::InvalidArgument,
        "File, capture and event log need distinct valid paths",
    ))?;
    Ok(())
}

fn validate_destination(path: &Path, overwrite: bool) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure!(
        parent.is_dir() && path.file_name().is_some() && !path.is_dir(),
        AppError::new(
            ErrorCode::Destination,
            "Choose a file in an existing folder"
        )
    );
    ensure!(
        overwrite || !path.exists(),
        AppError::new(
            ErrorCode::Destination,
            "Destination already exists; explicitly allow replacement or choose a new name"
        )
    );
    Ok(())
}

#[derive(Default)]
pub struct SessionControl {
    pub cancelled: Arc<AtomicBool>,
    committed: Mutex<Option<Completion>>,
}
impl SessionControl {
    pub fn cancel(&self) {
        // Serialize cancellation against the final filesystem commit, never
        // against DSP. A committed result always remains a committed result.
        let _guard = self.committed.lock().unwrap();
        self.cancelled.store(true, Ordering::Release);
    }
    fn received(&self, path: &Path, file: &VerifiedFile, overwrite: bool) -> Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut staged = tempfile::Builder::new()
            .prefix(".tonequill-")
            .suffix(".part")
            .tempfile_in(parent)?;
        staged.write_all(&file.bytes)?;
        staged.as_file().sync_all()?;
        let mut committed = self.committed.lock().unwrap();
        ensure!(
            !self.cancelled.load(Ordering::Acquire),
            "session cancelled before file commit"
        );
        if overwrite {
            staged.persist(path)?;
        } else {
            // Native no-replace commit refuses a file that appeared after the picker.
            staged.persist_noclobber(path)?;
        }
        *committed = Some(Completion::Received {
            file: file_info(&file.metadata),
            output_path: path.to_string_lossy().into_owned(),
        });
        Ok(())
    }
    fn result(&self) -> Option<Completion> {
        self.committed.lock().unwrap().clone()
    }
}

/// Adds a wall-clock/session bound to every worker-side audio operation, while
/// retaining the tested callback implementation. No Tauri work enters callbacks.
struct GovernedPort<P> {
    inner: P,
    control: Arc<SessionControl>,
    limit_ms: u64,
}
impl<P: AudioPort> GovernedPort<P> {
    fn health(&self) -> Result<()> {
        ensure!(
            !self.control.cancelled.load(Ordering::Acquire),
            "session cancelled"
        );
        ensure!(
            self.inner.now_ms() < self.limit_ms,
            AppError::new(ErrorCode::Timeout, "Session duration limit reached")
        );
        Ok(())
    }
}
impl<P: AudioPort> AudioPort for GovernedPort<P> {
    fn now_ms(&self) -> u64 {
        self.inner.now_ms()
    }
    fn read(&mut self, timeout: Duration) -> Result<InputEvent> {
        self.health()?;
        self.inner.read(timeout)
    }
    fn set_listening(&mut self, enabled: bool) {
        self.inner.set_listening(enabled);
    }
    fn write(&mut self, samples: &[f32]) -> Result<()> {
        self.health()?;
        self.inner.write(samples)
    }
    fn finish(&mut self) -> Result<()> {
        self.health()?;
        self.inner.finish()
    }
    fn pause(&mut self, duration: Duration) -> Result<()> {
        self.health()?;
        self.inner.pause(duration)
    }
}

/// The desktop and virtual-audio service tests enter this exact implementation.
/// Returns after finalizing diagnostic capture and dropping the owned audio port.
pub fn run_with_port<P: AudioPort>(
    request: &SessionRequest,
    port: P,
    control: Arc<SessionControl>,
    observer: Observer,
) -> Result<Completion> {
    let result = (|| {
        let log = EventLog::open(request.events_path.as_deref().map(Path::new)).context(
            AppError::new(
                ErrorCode::DiagnosticWrite,
                "Cannot create event log; choose a new path",
            ),
        )?;
        let port = GovernedPort {
            inner: port,
            control: control.clone(),
            limit_ms: u64::from(request.max_seconds) * 1000,
        };
        let mut link = Link::new(port, Timing::default(), log)?;
        link.set_observer(observer.clone());
        if request.direction == Direction::Receive {
            let mut capture = InputDiagnostics::new(request.capture_path.as_deref().map(Path::new))
                .context(AppError::new(
                    ErrorCode::DiagnosticWrite,
                    "Cannot create microphone capture; choose a new path",
                ))?;
            capture.interval_ms = 500;
            link.input_diagnostics = Some(capture);
            let mut commit_failure = None;
            let commit = |file: &VerifiedFile| {
                observer(Event::StateChanged {
                    phase: Phase::Verifying,
                });
                control
                    .received(Path::new(&request.path), file, request.overwrite)
                    .map_err(|error| {
                        let detail = error.to_string();
                        commit_failure = Some(error.context(AppError::new(
                            ErrorCode::Destination,
                            "Cannot commit the verified file",
                        )));
                        detail
                    })
            };
            let received = match request.mode {
                TransferMode::OneWay => runtime::one_way::receive(
                    &mut link,
                    u64::from(request.idle_timeout_seconds) * 1000,
                    commit,
                )
                .map(|_| ()),
                TransferMode::Reliable => runtime::run_receiver(
                    &mut link,
                    &mut ReceiveSession::default(),
                    u64::from(request.idle_timeout_seconds) * 1000,
                    400_000,
                    commit,
                ),
            };
            let finalized = link.finish_input_diagnostics();
            if let Some(error) = commit_failure {
                return Err(error);
            }
            received?;
            finalized.context(AppError::new(
                ErrorCode::DiagnosticWrite,
                "Cannot finalize microphone capture",
            ))?;
            control
                .result()
                .context("Receiver ended without a verified file")
        } else {
            let (name, bytes) = read_input(Path::new(&request.path))?;
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let id = nanos as u32
                ^ (nanos >> 32) as u32
                ^ NEXT_ID.fetch_add(1, Ordering::Relaxed) as u32
                ^ std::process::id();
            let fragments = Fragmenter::new(id, &name, &bytes)?;
            let file = file_info(fragments.metadata());
            ensure!(
                request
                    .expected_sha256
                    .as_ref()
                    .is_none_or(|expected| *expected == file.sha256),
                AppError::new(
                    ErrorCode::InvalidArgument,
                    "File changed after inspection; choose it again"
                )
            );
            ensure!(
                file.duration_seconds < f64::from(request.max_seconds),
                AppError::new(
                    ErrorCode::InvalidArgument,
                    "Estimated playback exceeds the selected session limit"
                )
            );
            observer(Event::MetadataReceived {
                transfer_id: format!("{id:08X}"),
                file: file.clone(),
            });
            match request.mode {
                TransferMode::OneWay => link.transmit(&fragments.collect::<Vec<_>>(), true)?,
                TransferMode::Reliable => {
                    let mut sender = SendSession::new(id, &name, &bytes, 8, 8)?;
                    runtime::run_sender(&mut link, &mut sender)?;
                }
            }
            Ok(Completion::Sent {
                file,
                peer_verified: request.mode == TransferMode::Reliable,
            })
        }
    })();
    // Audio and diagnostic objects in the closure have dropped before terminal
    // state reaches React. Later audio/Close errors cannot erase a verified file.
    if let Some(committed) = control.result() {
        if let Err(error) = result {
            observer(Event::Warning {
                error: failure(&error),
            });
        }
        Ok(committed)
    } else {
        result
    }
}

struct SessionData {
    snapshot: SessionSnapshot,
    events: VecDeque<SessionEvent>,
    origin: Instant,
}
impl SessionData {
    fn publish(&mut self, event: Event) {
        if !matches!(self.snapshot.status, SessionStatus::Active { .. }) {
            return;
        }
        self.snapshot.elapsed_ms = self.origin.elapsed().as_millis().min(u32::MAX as u128) as u32;
        let progress = &mut self.snapshot.progress;
        match &event {
            Event::StateChanged { phase } => {
                self.snapshot.status = SessionStatus::Active { phase: *phase }
            }
            Event::MetadataReceived { transfer_id, file } => {
                progress.transfer_id = Some(transfer_id.clone());
                progress.total_packets = Some(file.data_packets);
                progress.total_bytes = Some(file.bytes);
                self.snapshot.file = Some(file.clone());
            }
            Event::Progress { progress: incoming } => {
                progress.transfer_id = incoming.transfer_id.clone();
                progress.received_packets = incoming.received_packets;
                progress.total_packets = incoming.total_packets;
                progress.bytes_received = incoming.bytes_received;
                if incoming.total_bytes.is_some() {
                    progress.total_bytes = incoming.total_bytes;
                }
                progress.duplicate_frames = incoming.duplicate_frames;
                progress.retransmissions = incoming.retransmissions;
            }
            Event::Signal { metrics } => self.snapshot.signal = Some(metrics.clone()),
            Event::FrameDetected { metrics } => self.snapshot.frame = Some(metrics.clone()),
            Event::FrameAccepted { .. } => progress.valid_frames += 1,
            Event::FrameRejected { code, .. } => {
                progress.rejected_frames += 1;
                if *code == ErrorCode::CrcFailure {
                    progress.crc_failures += 1;
                }
            }
            Event::AudioGap { gaps, .. } => {
                self.snapshot.signal.get_or_insert_default().gaps = *gaps;
            }
            Event::FrameQueued { flags, .. } => {
                if *flags == tonequill_core::protocol::packet::FLAG_DATA {
                    progress.queued_packets += 1;
                }
            }
            Event::Completed { result } => {
                self.snapshot.status = SessionStatus::Completed {
                    result: result.clone(),
                }
            }
            Event::Failed { error } => {
                self.snapshot.status = SessionStatus::Failed {
                    error: error.clone(),
                }
            }
            Event::Cancelled => self.snapshot.status = SessionStatus::Cancelled,
            Event::RetransmissionRequested => (),
            Event::Warning { error } => {
                self.snapshot.warnings.push(error.clone());
                if self.snapshot.warnings.len() > 8 {
                    self.snapshot.warnings.remove(0);
                }
            }
        }
        self.snapshot.last_sequence += 1;
        self.events.push_back(SessionEvent {
            sequence: self.snapshot.last_sequence,
            elapsed_ms: self.snapshot.elapsed_ms,
            event,
        });
        if self.events.len() > EVENT_LIMIT {
            self.events.pop_front();
        }
    }
}

struct Running {
    id: String,
    control: Arc<SessionControl>,
    worker: JoinHandle<()>,
}
#[derive(Default)]
pub struct SessionManager {
    running: Mutex<Option<Running>>,
    data: Arc<Mutex<Option<SessionData>>>,
    closed: AtomicBool,
}
impl SessionManager {
    pub fn start(&self, request: SessionRequest) -> Result<String> {
        let limit = u64::from(request.max_seconds) * 1000;
        self.start_using(request, move |options, cancelled| {
            Audio::open_limited(options, cancelled, Some(limit))
        })
    }
    pub fn start_using<P, F>(&self, request: SessionRequest, factory: F) -> Result<String>
    where
        P: AudioPort + 'static,
        F: FnOnce(&AudioOptions, Arc<AtomicBool>) -> Result<P> + Send + 'static,
    {
        validate_request(&request)?;
        let mut running = self.running.lock().unwrap();
        ensure!(
            !self.closed.load(Ordering::Acquire),
            AppError::new(ErrorCode::Busy, "Application session service has closed")
        );
        ensure!(
            running.as_ref().is_none_or(|r| r.worker.is_finished()),
            AppError::new(ErrorCode::Busy, "Another audio session is still active")
        );
        if let Some(old) = running.take() {
            let _ = old.worker.join();
        }
        let id = format!(
            "{}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let control = Arc::new(SessionControl::default());
        *self.data.lock().unwrap() = Some(SessionData {
            origin: Instant::now(),
            events: VecDeque::new(),
            snapshot: SessionSnapshot {
                id: id.clone(),
                direction: request.direction,
                mode: request.mode,
                status: SessionStatus::Active {
                    phase: Phase::Preparing,
                },
                elapsed_ms: 0,
                progress: Progress::default(),
                file: None,
                signal: None,
                frame: None,
                input_device: request.devices.input_device.clone(),
                output_device: request.devices.output_device.clone(),
                input_channel: request.devices.input_channel,
                destination: (request.direction == Direction::Receive)
                    .then(|| request.path.clone()),
                capture_path: request.capture_path.clone(),
                events_path: request.events_path.clone(),
                last_sequence: 0,
                warnings: vec![],
            },
        });
        let data = self.data.clone();
        let observer: Observer = Arc::new(move |event| {
            if let Some(data) = data.lock().unwrap().as_mut() {
                data.publish(event);
            }
        });
        let worker_control = control.clone();
        let data = self.data.clone();
        let worker = thread::Builder::new()
            .name("tonequill-session".into())
            .spawn(move || {
                observer(Event::StateChanged {
                    phase: Phase::Preparing,
                });
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let options = AudioOptions {
                        input_device: request.devices.input_device.clone(),
                        output_device: request.devices.output_device.clone(),
                        input_channel: request.devices.input_channel as usize,
                        input_enabled: request.direction == Direction::Receive
                            || request.mode == TransferMode::Reliable,
                        output_enabled: request.direction == Direction::Send
                            || request.mode == TransferMode::Reliable,
                    };
                    let port = factory(&options, worker_control.cancelled.clone())?;
                    run_with_port(&request, port, worker_control.clone(), observer.clone())
                }))
                .unwrap_or_else(|_| {
                    Err(
                        AppError::new(ErrorCode::Internal, "Session worker stopped unexpectedly")
                            .into(),
                    )
                });
                match result {
                    Ok(result) => observer(Event::Completed { result }),
                    Err(_) if worker_control.cancelled.load(Ordering::Acquire) => {
                        observer(Event::Cancelled)
                    }
                    Err(error) => {
                        let mut error = failure(&error);
                        if error.code == ErrorCode::Timeout
                            && request.direction == Direction::Receive
                        {
                            let locked = data.lock().unwrap();
                            let progress = &locked.as_ref().unwrap().snapshot.progress;
                            error.code = if progress.total_packets.is_none() {
                                if progress.valid_frames > 0 {
                                    ErrorCode::MissingMetadata
                                } else if progress.crc_failures > 0 {
                                    ErrorCode::CrcFailure
                                } else {
                                    ErrorCode::NoFrames
                                }
                            } else {
                                ErrorCode::MissingPackets
                            };
                        }
                        observer(Event::Failed { error });
                    }
                }
            })
            .inspect_err(|error| {
                if let Some(data) = self.data.lock().unwrap().as_mut() {
                    data.publish(Event::Failed {
                        error: Failure {
                            code: ErrorCode::Internal,
                            detail: error.to_string(),
                        },
                    });
                }
            })?;
        *running = Some(Running {
            id: id.clone(),
            control,
            worker,
        });
        Ok(id)
    }
    pub fn poll(&self, after: u32) -> SessionUpdate {
        let data = self.data.lock().unwrap();
        let Some(data) = data.as_ref() else {
            return SessionUpdate {
                snapshot: None,
                events: vec![],
                events_truncated: false,
            };
        };
        let mut snapshot = data.snapshot.clone();
        if matches!(snapshot.status, SessionStatus::Active { .. }) {
            snapshot.elapsed_ms = data.origin.elapsed().as_millis().min(u32::MAX as u128) as u32;
        }
        SessionUpdate {
            snapshot: Some(snapshot),
            events: data
                .events
                .iter()
                .filter(|e| e.sequence > after)
                .cloned()
                .collect(),
            events_truncated: data
                .events
                .front()
                .is_some_and(|e| e.sequence > after.saturating_add(1)),
        }
    }
    pub fn cancel(&self, id: &str) -> Result<()> {
        let mut running = self.running.lock().unwrap();
        if let Some(active) = running.as_ref() {
            ensure!(
                active.id == id,
                AppError::new(
                    ErrorCode::InvalidArgument,
                    "Session ID does not match the active session"
                )
            );
            active.control.cancel();
        }
        if let Some(active) = running.take() {
            active
                .worker
                .join()
                .map_err(|_| AppError::new(ErrorCode::Internal, "Cannot join session worker"))?;
        }
        Ok(())
    }
    pub fn shutdown(&self) {
        let mut running = self.running.lock().unwrap();
        if let Some(active) = running.take() {
            active.control.cancel();
            let _ = active.worker.join();
        }
    }
    /// Permanently closes the service. Races with queued Tauri start commands
    /// are rejected even if that command was submitted before window close.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.shutdown();
    }
}
impl Drop for SessionManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests;
