use tonequill_core::transfer::reliable::simulation::{
    FaultAction, FrameKind, LossModel, ScriptedFault, SimulationConfig, Strategy, simulate,
};

fn data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i * 173 + i / 255 + 31) as u8).collect()
}
fn fault(kind: FrameKind, action: FaultAction) -> ScriptedFault {
    ScriptedFault {
        kind,
        sequence: None,
        occurrence: 1,
        action,
    }
}

#[test]
fn actual_state_machines_transfer_every_boundary_with_no_losses_or_retries() {
    for size in [0, 1, 256, 257, 1024, 10240] {
        let result = simulate(&data(size), &SimulationConfig::default()).unwrap();
        assert!(
            result.success && result.receiver_committed && result.sender_confirmed,
            "{result:?}"
        );
        assert_eq!(result.commits, 1);
        assert_eq!(result.sender.retransmitted_data, 0);
        assert_eq!(result.sender.timeouts, 0);
        assert_eq!(result.half_duplex_rejects, 0);
        assert_eq!(result.data_frames as usize, size.div_ceil(256));
        assert_eq!(result.useful_bytes as usize, size);
    }
}

#[test]
fn one_way_airtime_matches_existing_wav_without_artificial_batch_boundaries() {
    for (size, milliseconds) in [(1024, 97520), (10240, 911120)] {
        let config = SimulationConfig {
            strategy: Strategy::OneWay,
            ..Default::default()
        };
        let result = simulate(&data(size), &config).unwrap();
        assert!(result.success && result.receiver_committed && !result.sender_confirmed);
        assert_eq!(result.elapsed_ms, milliseconds);
        assert_eq!(result.modulation_ms + result.guard_ms, milliseconds);
        assert_eq!(result.control_frames, 0);
    }
}

#[test]
fn scripted_metadata_data_feedback_complete_loss_and_crc_failures_recover() {
    for scripted in [
        fault(FrameKind::Metadata, FaultAction::Drop),
        fault(FrameKind::Data, FaultAction::Drop),
        fault(FrameKind::Poll, FaultAction::Drop),
        fault(FrameKind::Status, FaultAction::Drop),
        fault(FrameKind::Complete, FaultAction::Drop),
        fault(FrameKind::Data, FaultAction::Corrupt),
        fault(FrameKind::Status, FaultAction::Corrupt),
        fault(FrameKind::Complete, FaultAction::Corrupt),
        fault(FrameKind::Metadata, FaultAction::Duplicate(10)),
        fault(FrameKind::Data, FaultAction::Duplicate(10)),
        fault(FrameKind::Status, FaultAction::Duplicate(50000)),
        fault(FrameKind::Data, FaultAction::Delay(30000)),
    ] {
        let config = SimulationConfig {
            faults: vec![scripted.clone()],
            ..Default::default()
        };
        let result = simulate(&data(1024), &config).unwrap();
        assert!(result.success, "fault={scripted:?}: {result:?}");
        assert_eq!(result.commits, 1);
        if scripted.action == FaultAction::Corrupt {
            assert!(result.crc_rejects >= 1);
        }
        if scripted.kind == FrameKind::Complete {
            assert_eq!(result.sender.retransmitted_data, 0);
        }
    }
}

#[test]
fn several_missing_packets_are_retransmitted_without_repeating_the_whole_window() {
    let config = SimulationConfig {
        faults: vec![
            ScriptedFault {
                sequence: Some(1),
                ..fault(FrameKind::Data, FaultAction::Drop)
            },
            ScriptedFault {
                sequence: Some(3),
                ..fault(FrameKind::Data, FaultAction::Drop)
            },
        ],
        ..Default::default()
    };
    let result = simulate(&data(1024), &config).unwrap();
    assert!(result.success);
    assert_eq!(result.sender.retransmitted_data, 2);
    assert_eq!(result.data_frames, 6);
}

#[test]
fn complete_blackouts_exhaust_retries_and_do_not_claim_delivery() {
    for loss in [
        LossModel {
            data_loss: 1.0,
            ..Default::default()
        },
        LossModel {
            control_loss: 1.0,
            ..Default::default()
        },
    ] {
        let result = simulate(
            &data(257),
            &SimulationConfig {
                retries: 3,
                loss,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!result.success && !result.sender_confirmed);
        assert!(result.retry_exhausted);
        assert_eq!(result.useful_bytes, 0);
        assert!(result.elapsed_ms < 1_000_000);
    }
}

#[test]
fn seeded_asymmetric_delay_and_duplicate_trials_are_reproducible() {
    for (data_loss, control_loss) in [(0.1, 0.02), (0.02, 0.1), (0.2, 0.1)] {
        let config = SimulationConfig {
            seed: 391,
            loss: LossModel {
                data_loss,
                control_loss,
                corruption: 0.01,
                duplication: 0.1,
                jitter_ms: 3500,
                ..Default::default()
            },
            ..Default::default()
        };
        let a = simulate(&data(10240), &config).unwrap();
        let b = simulate(&data(10240), &config).unwrap();
        assert_eq!(a, b);
        assert!(a.success, "{a:?}");
    }
}

#[test]
fn basic_reliability_campaign_uses_fixed_consecutive_seeds() {
    for loss in [0.05, 0.10] {
        let mut completed = 0;
        for seed in 1..=100 {
            let config = SimulationConfig {
                seed,
                loss: LossModel {
                    data_loss: loss,
                    control_loss: loss,
                    ..Default::default()
                },
                ..Default::default()
            };
            completed += u32::from(simulate(&data(10240), &config).unwrap().success);
        }
        assert!(completed >= 99, "loss={loss}: {completed}/100");
    }
}

#[test]
fn invalid_probabilities_and_windows_fail_without_running() {
    for probability in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
        let config = SimulationConfig {
            loss: LossModel {
                data_loss: probability,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(simulate(&[], &config).is_err());
    }
    assert!(
        simulate(
            &[],
            &SimulationConfig {
                strategy: Strategy::SelectiveRepeat { window: 0 },
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn lost_every_complete_can_leave_committed_receiver_but_never_sender_success() {
    let config = SimulationConfig {
        retries: 2,
        faults: (1..=3)
            .map(|occurrence| ScriptedFault {
                occurrence,
                ..fault(FrameKind::Complete, FaultAction::Drop)
            })
            .collect(),
        ..Default::default()
    };
    let result = simulate(&data(257), &config).unwrap();
    assert!(result.receiver_committed && result.retry_exhausted);
    assert!(!result.success && !result.sender_confirmed);
    assert_eq!(result.commits, 1);
    assert_eq!(result.sender.retransmitted_data, 0);
    assert_eq!(result.useful_bytes, 0);
}

#[test]
fn campaign_aggregates_airtime_and_failure_cost_without_success_bias() {
    use tonequill_core::transfer::reliable::evaluation::campaign;
    let config = SimulationConfig::default();
    let totals = campaign(1024, 3, 1, &config).unwrap();
    assert_eq!(totals.trials, 3);
    assert_eq!(totals.successful, 3);
    assert_eq!(totals.useful_bytes, 3072);
    assert_eq!(totals.data_frames, 12);
    assert_eq!(totals.elapsed_ms, 118960 * 3);
    assert!((totals.goodput_bytes_per_second() - 1024.0 / 118.960).abs() < 1e-10);
    assert_eq!(totals.modulation_ms, totals.on_air_bits * 10);
    let failed = campaign(
        1024,
        3,
        1,
        &SimulationConfig {
            loss: LossModel {
                control_loss: 1.0,
                ..Default::default()
            },
            ..config
        },
    )
    .unwrap();
    assert_eq!(failed.successful, 0);
    assert_eq!(failed.goodput_bytes_per_second(), 0.0);
    assert_eq!(failed.overhead_fraction(), 1.0);
    assert!(failed.elapsed_ms > 0);
    assert!(campaign(1, 0, 1, &SimulationConfig::default()).is_err());
    assert!(campaign(1, 2, u64::MAX, &SimulationConfig::default()).is_err());
}
