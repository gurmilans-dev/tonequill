use tonequill_core::{
    demodulation::bfsk::demodulate,
    demodulation::sync::find_frame_start,
    modulation::bfsk::modulate,
    protocol::{
        bitstream::{bits_to_bytes, bytes_to_bits},
        framing::{decode_frame, encode_frame},
        packet::Packet,
    },
};

#[test]
fn packet_survives_bfsk_roundtrip() {
    let original = Packet::new(0xDEADBEEF, 0, b"Hello from Tonequill!".to_vec());

    // Packet -> bytes
    let frame = encode_frame(&original).unwrap();

    // bytes -> bits
    let tx_bits = bytes_to_bits(&frame);

    // bits -> audio
    let samples = modulate(&tx_bits);

    // audio -> bits
    let rx_bits = demodulate(&samples);

    // bits -> bytes
    let rx_bytes = bits_to_bytes(&rx_bits).unwrap();

    // bytes -> Packet
    let received = decode_frame(&rx_bytes).unwrap();

    assert_eq!(original, received);
}
#[test]
fn binary_payload_survives_bfsk_roundtrip() {
    let payload: Vec<u8> = (0..=255).collect();

    let original = Packet::new(0xCAFEBABE, 7, payload);

    let frame = encode_frame(&original).unwrap();

    let tx_bits = bytes_to_bits(&frame);

    let samples = modulate(&tx_bits);

    let rx_bits = demodulate(&samples);

    let rx_bytes = bits_to_bytes(&rx_bits).unwrap();

    let received = decode_frame(&rx_bytes).unwrap();

    assert_eq!(original, received);
}
#[test]
fn synchronizes_after_leading_silence() {
    let original = Packet::new(0xAABBCCDD, 0, b"sync test".to_vec());

    let frame = encode_frame(&original).unwrap();

    let bits = bytes_to_bits(&frame);

    let signal = modulate(&bits);

    // Simuliamo una registrazione:
    //
    // 3457 sample di silenzio
    // +
    // packet Tonequill
    // +
    // silenzio finale

    let leading_silence = 3457;

    let mut recording = vec![0.0_f32; leading_silence];

    recording.extend_from_slice(&signal);

    recording.extend(std::iter::repeat_n(0.0_f32, 2000));

    let sync = find_frame_start(&recording).expect("preamble should be found");

    let difference = sync.start_sample.abs_diff(leading_silence);

    assert!(
        difference <= 8,
        "expected start near {}, found {}",
        leading_silence,
        sync.start_sample
    );
    assert!(
        sync.quality > 0.8,
        "expected strong preamble quality, got {}",
        sync.quality
    );
}
