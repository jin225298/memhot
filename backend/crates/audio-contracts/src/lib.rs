//! Platform-independent audio transport contracts.

use serde::{Deserialize, Serialize};

/// PCM sample format shared by capture clients and remote ASR adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

impl AudioFormat {
    pub fn standard_speech() -> Self {
        Self {
            sample_rate: 16_000,
            channels: 1,
            bits_per_sample: 16,
        }
    }

    pub fn high_quality() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 16,
        }
    }

    pub fn validate_pcm16(self, byte_len: usize) -> Result<(), &'static str> {
        if !(8_000..=192_000).contains(&self.sample_rate)
            || !(1..=2).contains(&self.channels)
            || self.bits_per_sample != 16
        {
            return Err("unsupported PCM16 format");
        }
        let frame_bytes = usize::from(self.channels) * 2;
        if byte_len == 0 || byte_len % frame_bytes != 0 || byte_len > u32::MAX as usize - 44 {
            return Err("invalid PCM16 payload length");
        }
        Ok(())
    }

    pub fn duration_ms(self, byte_len: usize) -> Option<u64> {
        self.validate_pcm16(byte_len).ok()?;
        (byte_len as u64)
            .checked_mul(1000)
            .map(|n| n / (u64::from(self.sample_rate) * u64::from(self.channels) * 2))
    }
}
