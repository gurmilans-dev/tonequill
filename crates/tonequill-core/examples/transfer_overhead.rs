//! Reproduce protocol overhead without generating minutes of audio.
use tonequill_core::transfer::{Fragmenter, waveform::TransmissionPlan};

fn main() {
    println!(
        "filename,file_bytes,data_packets,metadata_frames,total_frames,frame_bytes,modulated_bits,modulated_seconds,silence_seconds,total_seconds,wav_pcm_bytes"
    );
    for size in [1024, 10 * 1024] {
        let data = vec![0_u8; size];
        let fragments = Fragmenter::new(1, "sample.bin", &data).unwrap();
        let plan = TransmissionPlan::new(fragments.metadata()).unwrap();
        println!(
            "sample.bin,{size},{},{},{},{},{},{:.2},{:.2},{:.2},{}",
            plan.data_packets,
            plan.control_frames,
            plan.total_frames,
            plan.frame_bytes,
            plan.modulated_bits,
            plan.modulated_bits as f64 / 100.0,
            plan.silence_samples as f64 / 48000.0,
            plan.duration_seconds(),
            plan.total_samples * 2
        );
    }
}
