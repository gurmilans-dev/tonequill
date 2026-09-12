pub fn bytes_to_bits(bytes: &[u8]) -> Vec<bool> {
    let mut bits = Vec::with_capacity(bytes.len() * 8);

    for &byte in bytes {
        for shift in (0..8).rev() {
            let bit = ((byte >> shift) & 1) != 0;
            bits.push(bit);
        }
    }

    bits
}

pub fn bits_to_bytes(bits: &[bool]) -> Option<Vec<u8>> {
    if !bits.len().is_multiple_of(8) {
        return None;
    }

    let mut bytes = Vec::with_capacity(bits.len() / 8);

    for chunk in bits.as_chunks::<8>().0 {
        let mut byte = 0u8;

        for &bit in chunk {
            byte <<= 1;

            if bit {
                byte |= 1;
            }
        }

        bytes.push(byte);
    }

    Some(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_bit_roundtrip() {
        let original = vec![0x00, 0x01, 0xA5, 0xFF, 0x42];

        let bits = bytes_to_bits(&original);
        let recovered = bits_to_bytes(&bits).unwrap();

        assert_eq!(original, recovered);
    }

    #[test]
    fn rejects_non_byte_aligned_bits() {
        let bits = vec![true, false, true];

        assert!(bits_to_bytes(&bits).is_none());
    }
}
