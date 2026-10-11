//! Windows Process Loopback capture boundary.
//!
//! Target product implementation is Rust/Win32, not a .NET sidecar.
//! The proven C# behavior is frozen under reference/le-audio-relay.

pub mod process_loopback;

pub const SAMPLE_RATE_HZ: u32 = 48_000;
pub const CHANNELS: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    Float32Le,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureFormat {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
}

pub const CAPTURE_FORMAT: CaptureFormat = CaptureFormat {
    sample_rate_hz: SAMPLE_RATE_HZ,
    channels: CHANNELS,
    sample_format: SampleFormat::Float32Le,
};
