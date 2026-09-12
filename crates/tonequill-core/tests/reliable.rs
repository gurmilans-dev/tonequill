use tonequill_core::{
    protocol::{
        framing::{decode_frame, encode_frame},
        packet::{FLAG_DATA, FLAG_REQUEST, Packet},
    },
    transfer::reliable::{
        ReceiveSession, ReliableError, Request, RequestKind, SendSession, Status, StatusKind,
    },
};

#[test]
fn repeatedly_restarting_receiver_cannot_extend_transfer_forever() {
    let mut sender = SendSession::new(7, "x.bin", b"x", 1, 2).unwrap();
    for restart in 0..3 {
        let handshake = sender.next_burst().unwrap();
        let request = Request::decode(handshake.last().unwrap()).unwrap();
        sender
            .accept(
                &Status {
                    request,
                    received: 0,
                    kind: StatusKind::Receiving,
                }
                .packet()
                .unwrap(),
            )
            .unwrap();
        let burst = sender.next_burst().unwrap();
        let request = Request::decode(burst.last().unwrap()).unwrap();
        let result = sender.accept(
            &Status {
                request,
                received: 0,
                kind: StatusKind::NeedMetadata,
            }
            .packet()
            .unwrap(),
        );
        if restart < 2 {
            assert_eq!(result, Ok(true));
        } else {
            assert_eq!(result, Err(ReliableError::RetryLimit));
        }
    }
    assert_eq!(sender.next_burst(), Err(ReliableError::RetryLimit));
}

#[test]
fn control_payload_has_fixed_network_byte_order_and_exact_lengths() {
    let request = Request {
        transfer_id: 0x12345678,
        round: 0x01020304,
        base: 0x1234,
        count: 8,
        kind: RequestKind::Poll,
    };
    assert_eq!(
        request.packet().unwrap().payload,
        [b'S', b'L', b'A', b'R', 1, 1, 1, 2, 3, 4, 0x12, 0x34, 8]
    );
    let status = Status {
        request,
        received: 0x81,
        kind: StatusKind::Receiving,
    }
    .packet()
    .unwrap();
    assert_eq!(&status.payload[13..], &[0, 0, 0, 0x81]);
    assert_eq!(Status::decode(&wire(&status)).unwrap().received, 0x81);
    for length in 0..=256 {
        if length == 17 {
            continue;
        }
        let mut malformed = status.clone();
        malformed.payload.resize(length, 0);
        assert!(Status::decode(&malformed).is_err(), "length {length}");
    }
    let complete = Status {
        request,
        received: 255,
        kind: StatusKind::Complete([0xa5; 32]),
    }
    .packet()
    .unwrap();
    assert_eq!(encode_frame(&complete).unwrap().len(), 73);
    assert_eq!(encode_frame(&request.packet().unwrap()).unwrap().len(), 37);
    for kind in [
        StatusKind::NeedMetadata,
        StatusKind::Failed,
        StatusKind::Busy,
        StatusKind::Complete([0; 32]),
    ] {
        assert!(
            Status {
                request,
                received: 1,
                kind
            }
            .packet()
            .is_err()
        );
    }
}

#[test]
fn injected_time_deadlines_and_terminal_states_do_not_wait_on_wall_clock() {
    use tonequill_core::transfer::reliable::{ReceiverState, SenderState, Timing};
    let mut sender = SendSession::new(1, "x.bin", b"x", 1, 1).unwrap();
    sender
        .set_timing(Timing {
            feedback_timeout_ms: 100,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(sender.state(), SenderState::ReadyMetadata);
    sender.next_burst().unwrap();
    assert_eq!(sender.state(), SenderState::WaitingMetadataAck);
    sender.transmitted(7000).unwrap();
    assert_eq!(sender.deadline_ms(), Some(7100));
    assert_eq!(sender.tick(7099).unwrap(), None);
    assert!(sender.tick(7100).unwrap().is_some());
    assert_eq!(sender.tick(9999).unwrap(), None); // Retry is still being transmitted.
    sender.transmitted(10_000).unwrap();
    assert_eq!(sender.tick(10_100), Err(ReliableError::RetryLimit));
    assert_eq!(sender.state(), SenderState::Failed);
    assert_eq!(sender.next_burst(), Err(ReliableError::RetryLimit));
    assert_eq!(sender.statistics().timeouts, 2);
    let mut cancelled = SendSession::new(1, "x.bin", b"x", 1, 1).unwrap();
    cancelled.cancel();
    assert_eq!(cancelled.next_burst(), Err(ReliableError::Cancelled));
    let mut receiver = ReceiveSession::default();
    receiver.cancel();
    assert_eq!(receiver.state(), ReceiverState::Failed);
    let mut active_sender = SendSession::new(1, "x.bin", b"x", 1, 1).unwrap();
    for packet in active_sender.next_burst().unwrap() {
        if let Some(reply) = receiver
            .accept(packet, |_| panic!("cancelled receiver committed"))
            .unwrap()
        {
            assert_eq!(Status::decode(&reply).unwrap().kind, StatusKind::Failed);
        }
    }
    assert_eq!(receiver.state(), ReceiverState::Failed);
}

#[test]
fn conflicting_duplicate_poisoning_and_interleaved_session_ids_fail_closed() {
    use tonequill_core::transfer::Fragmenter;
    use tonequill_core::transfer::reliable::ReceiverState;
    let frames: Vec<_> = Fragmenter::new(1, "x.bin", b"first").unwrap().collect();
    let mut receiver = ReceiveSession::default();
    for packet in &frames {
        receiver.accept(wire(packet), |_| panic!()).unwrap();
    }
    // Identical metadata and data repetitions remain idempotent.
    for packet in frames.iter().rev() {
        receiver.accept(wire(packet), |_| panic!()).unwrap();
    }
    let foreign = Request {
        transfer_id: 2,
        round: 1,
        base: 0,
        count: 0,
        kind: RequestKind::Poll,
    };
    let busy = receiver
        .accept(foreign.packet().unwrap(), |_| panic!())
        .unwrap()
        .unwrap();
    assert_eq!(Status::decode(&busy).unwrap().kind, StatusKind::Busy);
    assert_eq!(receiver.transfer_id(), Some(1));
    let impossible = Request {
        transfer_id: 1,
        count: 2,
        ..foreign
    };
    assert!(
        receiver
            .accept(impossible.packet().unwrap(), |_| panic!())
            .is_err()
    );
    let mut conflict = frames[1].clone();
    conflict.payload[0] ^= 1;
    receiver.accept(wire(&conflict), |_| panic!()).unwrap();
    assert_eq!(receiver.state(), ReceiverState::Failed);
    let poll = Request {
        transfer_id: 1,
        ..foreign
    };
    let status = receiver
        .accept(poll.packet().unwrap(), |_| {
            panic!("conflict reached commit")
        })
        .unwrap()
        .unwrap();
    assert_eq!(Status::decode(&status).unwrap().kind, StatusKind::Failed);
    assert!(!receiver.is_committed());
}

#[test]
fn duplicate_and_reordered_data_complete_once_and_stale_close_is_ignored() {
    use tonequill_core::transfer::Fragmenter;
    let data = vec![0x8f; 513];
    let frames: Vec<_> = Fragmenter::new(1, "x.bin", &data).unwrap().collect();
    let mut receiver = ReceiveSession::default();
    receiver.accept(frames[0].clone(), |_| panic!()).unwrap();
    for packet in frames[1..].iter().rev() {
        receiver.accept(packet.clone(), |_| panic!()).unwrap();
    }
    let poll = Request {
        transfer_id: 1,
        round: 7,
        base: 0,
        count: 3,
        kind: RequestKind::Poll,
    };
    let mut commits = 0;
    let status = receiver
        .accept(poll.packet().unwrap(), |file| {
            assert_eq!(file.bytes, data);
            commits += 1;
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert!(matches!(
        Status::decode(&status).unwrap().kind,
        StatusKind::Complete(_)
    ));
    let close = Request {
        kind: RequestKind::Close,
        round: 6,
        ..poll
    };
    receiver
        .accept(close.packet().unwrap(), |_| panic!())
        .unwrap();
    assert!(!receiver.is_closed());
    receiver
        .accept(poll.packet().unwrap(), |_| panic!("second commit"))
        .unwrap();
    receiver
        .accept(
            Request { round: 7, ..close }.packet().unwrap(),
            |_| panic!(),
        )
        .unwrap();
    assert!(receiver.is_closed());
    assert_eq!(commits, 1);
}

fn wire(packet: &Packet) -> Packet {
    decode_frame(&encode_frame(packet).unwrap()).unwrap()
}

#[test]
fn full_sessions_cover_empty_boundaries_binary_and_ten_kib() {
    for size in [0, 1, 256, 257, 1024, 10240] {
        let data: Vec<u8> = (0..size).map(|i| (i * 71 + 9) as u8).collect();
        let mut sender = SendSession::new(0xdeadbeef, "file.bin", &data, 8, 8).unwrap();
        let mut receiver = ReceiveSession::default();
        let mut commits = Vec::new();
        for _ in 0..100 {
            for packet in sender.next_burst().unwrap() {
                if let Some(status) = receiver
                    .accept(wire(&packet), |file| {
                        commits.push(file.bytes.clone());
                        Ok(())
                    })
                    .unwrap()
                {
                    assert!(sender.accept(&wire(&status)).unwrap());
                }
            }
            if sender.is_complete() {
                break;
            }
        }
        assert!(sender.is_complete(), "size={size}");
        assert_eq!(commits.as_slice(), std::slice::from_ref(&data));
        assert_eq!(sender.statistics().retransmitted_data, 0);
        receiver
            .accept(wire(&sender.close_packet().unwrap()), |_| {
                panic!("second commit")
            })
            .unwrap();
        assert!(receiver.is_closed());
    }
}

#[test]
fn lost_data_are_selectively_retransmitted_and_lost_status_only_repolls() {
    let data = vec![0x9e; 1024];
    let mut sender = SendSession::new(1, "x.bin", &data, 4, 8).unwrap();
    let mut receiver = ReceiveSession::default();
    for packet in sender.next_burst().unwrap() {
        if let Some(status) = receiver
            .accept(wire(&packet), |_| panic!("not complete"))
            .unwrap()
        {
            sender.accept(&status).unwrap();
        }
    }
    let first = sender.next_burst().unwrap();
    for packet in &first {
        if packet.flags == FLAG_DATA && packet.sequence == 1 {
            continue;
        }
        // Deliberately discard the status response too.
        receiver
            .accept(wire(packet), |_| panic!("missing packet"))
            .unwrap();
    }
    let poll = sender.retry_poll().unwrap();
    assert_eq!(poll.flags, FLAG_REQUEST);
    assert_eq!(sender.statistics().data_transmissions, 4);
    let status = receiver
        .accept(wire(&poll), |_| panic!("missing packet"))
        .unwrap()
        .unwrap();
    assert_eq!(Status::decode(&status).unwrap().received, 0b1101);
    sender.accept(&wire(&status)).unwrap();
    let repair = sender.next_burst().unwrap();
    assert_eq!(repair.len(), 2);
    assert_eq!(repair[0].sequence, 1);
    let mut commits = 0;
    let mut last_status = None;
    for packet in repair {
        last_status = receiver
            .accept(wire(&packet), |file| {
                assert_eq!(file.bytes, data);
                commits += 1;
                Ok(())
            })
            .unwrap();
    }
    // Lose the final committed ACK; retry cannot cause another write.
    let status = receiver
        .accept(wire(&sender.retry_poll().unwrap()), |_| {
            panic!("duplicate commit")
        })
        .unwrap()
        .unwrap();
    assert_eq!(status, last_status.unwrap());
    sender.accept(&status).unwrap();
    assert!(sender.is_complete());
    assert_eq!(commits, 1);
    assert_eq!(sender.statistics().retransmitted_data, 1);
}

#[test]
fn missing_metadata_and_receiver_restart_recover() {
    let data = vec![0x37; 513];
    let mut sender = SendSession::new(1, "x.bin", &data, 2, 8).unwrap();
    let mut receiver = ReceiveSession::default();
    let burst = sender.next_burst().unwrap();
    let status = receiver
        .accept(wire(burst.last().unwrap()), |_| panic!())
        .unwrap()
        .unwrap();
    assert_eq!(
        Status::decode(&status).unwrap().kind,
        StatusKind::NeedMetadata
    );
    sender.accept(&status).unwrap();
    for packet in sender.next_burst().unwrap() {
        if let Some(status) = receiver.accept(wire(&packet), |_| panic!()).unwrap() {
            sender.accept(&status).unwrap();
        }
    }
    // A restart also invalidates data acknowledgments from the previous peer.
    for packet in sender.next_burst().unwrap() {
        if let Some(status) = receiver.accept(wire(&packet), |_| panic!()).unwrap() {
            sender.accept(&status).unwrap();
        }
    }
    assert_eq!(sender.statistics().acknowledged_packets, 2);
    receiver = ReceiveSession::default();
    for packet in sender.next_burst().unwrap() {
        if let Some(status) = receiver.accept(wire(&packet), |_| panic!()).unwrap() {
            sender.accept(&status).unwrap();
        }
    }
    assert_eq!(sender.statistics().acknowledged_packets, 0);
    let mut committed = false;
    for _ in 0..10 {
        for packet in sender.next_burst().unwrap() {
            if let Some(status) = receiver
                .accept(wire(&packet), |file| {
                    assert_eq!(file.bytes, data);
                    committed = true;
                    Ok(())
                })
                .unwrap()
            {
                sender.accept(&status).unwrap();
            }
        }
        if sender.is_complete() {
            break;
        }
    }
    assert!(committed && sender.is_complete());
}

#[test]
fn failed_commit_and_invalid_file_hash_never_send_success() {
    for bad_hash in [false, true] {
        let mut sender = SendSession::new(1, "x.bin", b"payload", 1, 8).unwrap();
        let mut receiver = ReceiveSession::default();
        for packet in sender.next_burst().unwrap() {
            if let Some(status) = receiver.accept(packet, |_| panic!()).unwrap() {
                sender.accept(&status).unwrap();
            }
        }
        for mut packet in sender.next_burst().unwrap() {
            if bad_hash && packet.flags == FLAG_DATA {
                packet.payload[0] ^= 1;
            }
            if let Some(status) = receiver
                .accept(wire(&packet), |_| {
                    assert!(!bad_hash, "hash-invalid bytes reached the commit callback");
                    Err("disk full".into())
                })
                .unwrap()
            {
                assert_eq!(Status::decode(&status).unwrap().kind, StatusKind::Failed);
                assert!(matches!(
                    sender.accept(&status),
                    Err(ReliableError::Rejected(_))
                ));
            }
        }
        assert!(!receiver.is_committed() && !sender.is_complete());
    }
}

#[test]
fn stale_unrelated_and_wrong_hash_status_cannot_confirm_delivery() {
    let mut sender = SendSession::new(7, "x.bin", b"x", 1, 3).unwrap();
    let burst = sender.next_burst().unwrap();
    let request = Request::decode(burst.last().unwrap()).unwrap();
    let mut status = Status {
        request,
        received: 0,
        kind: StatusKind::Receiving,
    };
    status.request.round += 1;
    assert!(!sender.accept(&status.packet().unwrap()).unwrap());
    status.request = request;
    status.request.transfer_id += 1;
    assert!(!sender.accept(&status.packet().unwrap()).unwrap());
    status.request = request;
    status.kind = StatusKind::Complete([0; 32]);
    assert!(sender.accept(&status.packet().unwrap()).is_err());
    assert!(!sender.is_complete());
}

#[test]
fn timeouts_and_receipt_without_committed_hash_are_bounded() {
    let mut sender = SendSession::new(1, "x.bin", b"", 1, 2).unwrap();
    sender.next_burst().unwrap();
    sender.retry_poll().unwrap();
    sender.retry_poll().unwrap();
    assert_eq!(sender.retry_poll(), Err(ReliableError::RetryLimit));
    let mut sender = SendSession::new(1, "x.bin", b"", 1, 2).unwrap();
    let mut stopped = false;
    for _ in 0..6 {
        let burst = sender.next_burst().unwrap();
        let request = Request::decode(burst.last().unwrap()).unwrap();
        let status = Status {
            request,
            received: 0,
            kind: StatusKind::Receiving,
        }
        .packet()
        .unwrap();
        if sender.accept(&status) == Err(ReliableError::RetryLimit) {
            stopped = true;
            break;
        }
    }
    assert!(stopped && !sender.is_complete());
}

#[test]
fn malformed_controls_and_unknown_versions_are_rejected() {
    let request = Request {
        transfer_id: 1,
        round: 1,
        base: 65504,
        count: 32,
        kind: RequestKind::Poll,
    };
    let packet = request.packet().unwrap();
    assert_eq!(Request::decode(&wire(&packet)).unwrap(), request);
    let status = Status {
        request,
        received: u32::MAX,
        kind: StatusKind::Complete([0xaa; 32]),
    };
    assert_eq!(
        Status::decode(&wire(&status.packet().unwrap())).unwrap(),
        status
    );
    for original in [packet, status.packet().unwrap()] {
        for length in 0..original.payload.len() {
            let mut bad = original.clone();
            bad.payload.truncate(length);
            assert!(if bad.flags == FLAG_REQUEST {
                Request::decode(&bad).is_err()
            } else {
                Status::decode(&bad).is_err()
            });
        }
        for (index, value) in [(0, 0), (4, 2), (5, 255), (12, 33)] {
            let mut bad = original.clone();
            bad.payload[index] = value;
            assert!(if bad.flags == FLAG_REQUEST {
                Request::decode(&bad).is_err()
            } else {
                Status::decode(&bad).is_err()
            });
        }
    }
    assert!(
        Request {
            base: 65535,
            count: 2,
            ..request
        }
        .packet()
        .is_err()
    );
    assert!(
        Status {
            request: Request {
                base: 0,
                count: 1,
                ..request
            },
            received: 2,
            kind: StatusKind::Receiving
        }
        .packet()
        .is_err()
    );
}
