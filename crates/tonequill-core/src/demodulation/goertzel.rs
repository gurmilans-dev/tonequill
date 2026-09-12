use std::f32::consts::PI;

pub fn power(samples: &[f32], target_frequency: f32, sample_rate: u32) -> f32 {
    let omega = 2.0 * PI * target_frequency / sample_rate as f32;

    let coefficient = 2.0 * omega.cos();

    let mut previous = 0.0_f32;
    let mut previous_two = 0.0_f32;

    for &sample in samples {
        let current = sample + coefficient * previous - previous_two;

        previous_two = previous;
        previous = current;
    }

    let real = previous - previous_two * omega.cos();

    let imag = previous_two * omega.sin();

    real * real + imag * imag
}
