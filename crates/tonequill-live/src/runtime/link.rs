use crate::audio::{AudioPort, InputEvent};
use crate::events::{ErrorCode, Event, FrameMetrics, Observer, Phase};
use anyhow::Result;
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::Path,
    time::Duration,
};
use tonequill_core::{
    config::SAMPLE_RATE,
    protocol::{
        framing::FrameError,
        packet::{FLAG_DATA, FLAG_METADATA, FLAG_REQUEST, FLAG_STATUS, Packet},
    },
    receiver::{LiveDecoder, ReceiveError},
    transfer::{
        reliable::{Duplex, Request, Status, Timing, TurnState},
        waveform::frame_samples,
    },
};

#[derive(Default, Debug)]
pub struct LinkStatistics {
    pub transmitted_frames: u64,
    pub control_frames: u64,
    pub received_frames: u64,
    pub crc_rejects: u64,
    pub other_phy_rejects: u64,
    pub invalid_controls: u64,
    pub capture_gaps: u64,
    pub modulation_ms: u64,
}

pub struct EventLog(Option<BufWriter<std::fs::File>>);
impl EventLog {
    pub fn open(path: Option<&Path>) -> Result<Self> {
        let mut log = Self(
            path.map(|path| {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map(BufWriter::new)
            })
            .transpose()?,
        );
        if let Some(writer) = &mut log.0 {
            writeln!(writer, "elapsed_ms,event,transfer_id,flags,sequence,detail")?;
            writer.flush()?;
        }
        Ok(log)
    }
    pub fn event(
        &mut self,
        now: u64,
        event: &str,
        packet: Option<&Packet>,
        detail: &str,
    ) -> Result<()> {
        if let Some(writer) = &mut self.0 {
            let (id, flags, sequence) =
                packet.map_or((String::new(), String::new(), String::new()), |p| {
                    (
                        format!("{:08X}", p.transfer_id),
                        format!("{:02X}", p.flags),
                        p.sequence.to_string(),
                    )
                });
            writeln!(
                writer,
                "{now},{event},{id},{flags},{sequence},\"{}\"",
                detail.replace('"', "\"\"").replace(['\r', '\n'], " ")
            )?;
            writer.flush()?;
        }
        Ok(())
    }
}

type Reporter = Box<dyn Fn(&str) + Send>;

pub struct Link<P: AudioPort> {
    pub port: P,
    pub statistics: LinkStatistics,
    pub log: EventLog,
    pub input_diagnostics: Option<super::capture::InputDiagnostics>,
    reporter: Option<Reporter>,
    observer: Option<Observer>,
    decoder: LiveDecoder,
    turn: Duplex,
    timing: Timing,
}
impl<P: AudioPort> Link<P> {
    pub fn new(port: P, timing: Timing, log: EventLog) -> Result<Self> {
        Ok(Self {
            port,
            statistics: LinkStatistics::default(),
            log,
            input_diagnostics: None,
            reporter: None,
            observer: None,
            decoder: LiveDecoder::default(),
            turn: Duplex::new(timing)?,
            timing,
        })
    }
    pub fn set_reporter(&mut self, reporter: impl Fn(&str) + Send + 'static) {
        self.reporter = Some(Box::new(reporter));
    }
    pub fn set_observer(&mut self, observer: Observer) {
        self.observer = Some(observer);
    }
    pub fn notify(&self, event: Event) {
        if let Some(observer) = &self.observer {
            observer(event);
        }
    }
    pub fn message(&self, message: String) {
        if let Some(reporter) = &self.reporter {
            reporter(&message);
        }
    }
    pub fn now_ms(&self) -> u64 {
        self.port.now_ms()
    }
    pub fn event(&mut self, event: &str, detail: &str) -> Result<()> {
        self.log.event(self.now_ms(), event, None, detail)
    }
    fn wait_turn(&mut self) -> Result<()> {
        if let Some(deadline) = self.turn.deadline_ms() {
            self.port.pause(Duration::from_millis(
                deadline.saturating_sub(self.now_ms()),
            ))?;
            self.turn.tick(self.now_ms());
        }
        Ok(())
    }
    fn silence(&mut self, milliseconds: u64) -> Result<()> {
        let silence = [0.0_f32; 4800];
        let mut remaining = milliseconds * u64::from(SAMPLE_RATE) / 1000;
        while remaining > 0 {
            let count = remaining.min(silence.len() as u64) as usize;
            self.port.write(&silence[..count])?;
            remaining -= count as u64;
        }
        Ok(())
    }
    pub fn transmit(&mut self, packets: &[Packet], initial: bool) -> Result<()> {
        if packets.is_empty() {
            return Ok(());
        }
        self.notify(Event::StateChanged {
            phase: Phase::Transmitting,
        });
        self.turn.begin(self.now_ms(), !initial)?;
        self.port.set_listening(false);
        self.decoder.reset();
        self.event("turnaround", "microphone gated; discard partial self audio")?;
        self.wait_turn()?;
        debug_assert_eq!(self.turn.state(), TurnState::Transmitting);
        self.silence(self.timing.edge_padding_ms)?;
        for (index, packet) in packets.iter().enumerate() {
            if index != 0 {
                self.silence(self.timing.inter_frame_guard_ms)?;
            }
            self.log
                .event(self.now_ms(), "tx_queued", Some(packet), &describe(packet))?;
            // At most one modulated frame is allocated, never a whole file/window.
            self.port.write(&frame_samples(packet)?)?;
            self.notify(Event::FrameQueued {
                transfer_id: format!("{:08X}", packet.transfer_id),
                flags: packet.flags,
                sequence: packet.sequence,
                bytes: packet.payload.len() as u32,
            });
            self.statistics.transmitted_frames += 1;
            self.statistics.control_frames +=
                u64::from(matches!(packet.flags, FLAG_REQUEST | FLAG_STATUS));
            self.statistics.modulation_ms += self.timing.frame_ms(packet)?;
        }
        self.silence(self.timing.edge_padding_ms)?;
        self.port.finish()?;
        self.turn.playback_finished(self.now_ms())?;
        self.event("settling", "output drained; suppress residual self echo")?;
        self.wait_turn()?;
        debug_assert_eq!(self.turn.state(), TurnState::Listening);
        self.port.set_listening(true);
        self.notify(Event::StateChanged {
            phase: Phase::Listening,
        });
        self.event("listening", "turn available for acoustic feedback")?;
        Ok(())
    }
    pub fn packets(&mut self) -> Result<Vec<Packet>> {
        let mut packets = Vec::new();
        match self.port.read(Duration::from_millis(30))? {
            InputEvent::Samples(samples) => {
                if let Some(diagnostics) = &mut self.input_diagnostics {
                    diagnostics.observe(&samples)?;
                }
                for attempt in self.decoder.push(&samples)? {
                    self.notify(Event::FrameDetected {
                        metrics: FrameMetrics {
                            start_sample: attempt.acquisition.start_sample,
                            acquisition_quality: attempt.acquisition.quality,
                            tone_concentration: attempt.acquisition.tone_concentration,
                            carrier_powers: attempt.acquisition.carrier_power,
                            decision_ratio: attempt.acquisition.decision_ratio,
                            samples_per_symbol: attempt.final_samples_per_symbol,
                            clock_ppm: attempt.acquisition.clock_estimated.then_some(
                                (attempt.acquisition.samples_per_symbol / 480.0 - 1.0) * 1e6,
                            ),
                            acquisition_window_samples: attempt
                                .acquisition
                                .acquisition_window_samples
                                as u32,
                        },
                    });
                    match attempt.result {
                        Ok(packet) => {
                            self.notify(Event::FrameAccepted {
                                transfer_id: format!("{:08X}", packet.transfer_id),
                                flags: packet.flags,
                                sequence: packet.sequence,
                                bytes: packet.payload.len() as u32,
                            });
                            self.statistics.received_frames += 1;
                            self.log.event(
                                self.now_ms(),
                                "rx_valid",
                                Some(&packet),
                                &describe(&packet),
                            )?;
                            packets.push(packet);
                        }
                        Err(error) => {
                            self.notify(Event::FrameRejected {
                                code: if matches!(
                                    error,
                                    ReceiveError::Frame(FrameError::CrcMismatch { .. })
                                ) {
                                    ErrorCode::CrcFailure
                                } else {
                                    ErrorCode::Integrity
                                },
                                detail: error.to_string(),
                            });
                            if matches!(error, ReceiveError::Frame(FrameError::CrcMismatch { .. }))
                            {
                                self.statistics.crc_rejects += 1;
                            } else {
                                self.statistics.other_phy_rejects += 1;
                            }
                            self.event("phy_rejected", &error.to_string())?;
                        }
                    }
                }
            }
            InputEvent::Gap => {
                self.statistics.capture_gaps += 1;
                self.notify(Event::AudioGap {
                    gaps: self.statistics.capture_gaps as u32,
                    sample: self
                        .input_diagnostics
                        .as_ref()
                        .map_or(0.0, |d| d.samples as f64),
                });
                self.decoder.reset();
                self.event(
                    "capture_gap",
                    &format!("capture_sample={}; partial audio discarded; missing frames require retransmission or replay",
                        self.input_diagnostics.as_ref().map_or(0, |d| d.samples)),
                )?;
                self.message("Microphone discontinuity/overrun: discarded partial audio; missing frames need retransmission or replay".to_string());
            }
            InputEvent::Discontinuity => {
                self.statistics.capture_gaps += 1;
                self.notify(Event::AudioGap {
                    gaps: self.statistics.capture_gaps as u32,
                    sample: self
                        .input_diagnostics
                        .as_ref()
                        .map_or(0.0, |d| d.samples as f64),
                });
                // WASAPI supplies the buffer carrying DATA_DISCONTINUITY after
                // its error callback. Retain all delivered PCM and rely on the
                // frame CRC; resetting here makes frequent driver flags prevent
                // every multi-second frame from ever completing.
                self.event(
                    "backend_discontinuity",
                    &format!(
                        "capture_sample={}; delivered PCM retained; CRC still required",
                        self.input_diagnostics.as_ref().map_or(0, |d| d.samples)
                    ),
                )?;
                self.message(
                    "Microphone backend discontinuity: delivered audio retained; frame CRC still protects integrity"
                        .to_string(),
                );
            }
            InputEvent::Idle => (),
        }
        self.report_input(false)?;
        Ok(packets)
    }
    fn report_input(&mut self, force: bool) -> Result<()> {
        let now = self.now_ms();
        if let Some(diagnostics) = &mut self.input_diagnostics
            && let Some((levels, mut metrics)) = diagnostics.report_with_metrics(now, force)?
        {
            metrics.gaps = self.statistics.capture_gaps as u32;
            self.notify(Event::Signal { metrics });
            let detail = format!(
                "{levels}; valid_frames={}; rejected={}; gaps={}",
                self.statistics.received_frames,
                self.statistics.crc_rejects + self.statistics.other_phy_rejects,
                self.statistics.capture_gaps
            );
            self.message(format!("Listening: {detail}"));
            self.event("audio_levels", &detail)?;
        }
        Ok(())
    }
    pub fn finish_input_diagnostics(&mut self) -> Result<()> {
        // Finalize even when level logging fails, so Ctrl+C retains a readable WAV.
        let report = self.report_input(true);
        let finalize = self
            .input_diagnostics
            .as_mut()
            .map_or(Ok(()), |d| d.finish());
        finalize?;
        report
    }
    pub fn invalid_control(&mut self, detail: &str) -> Result<()> {
        self.statistics.invalid_controls += 1;
        self.event("control_rejected", detail)
    }
    pub fn summary(&mut self, useful_bytes: u64, confirmed: bool) -> Result<()> {
        let seconds = self.now_ms() as f64 / 1000.0;
        let goodput = if confirmed && seconds > 0.0 {
            useful_bytes as f64 / seconds
        } else {
            0.0
        };
        let detail = format!(
            "confirmed={confirmed}; elapsed={seconds:.3}s; useful_goodput={goodput:.3} B/s; {:?}",
            self.statistics
        );
        self.message(detail.to_string());
        self.event("summary", &detail)
    }
}

fn describe(packet: &Packet) -> String {
    match packet.flags {
        FLAG_REQUEST => Request::decode(packet).map_or_else(
            |e| e.to_string(),
            |r| {
                format!(
                    "{:?}; round={}; base={}; count={}",
                    r.kind, r.round, r.base, r.count
                )
            },
        ),
        FLAG_STATUS => Status::decode(packet).map_or_else(
            |e| e.to_string(),
            |s| {
                format!(
                    "{:?}; round={}; base={}; count={}; bitmap={:08X}",
                    s.kind, s.request.round, s.request.base, s.request.count, s.received
                )
            },
        ),
        FLAG_DATA => format!("data; bytes={}", packet.payload.len()),
        FLAG_METADATA => format!("metadata; bytes={}", packet.payload.len()),
        _ => "legacy frame".into(),
    }
}
