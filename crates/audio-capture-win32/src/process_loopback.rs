//! Rust target for the LE-Audio-Relay ProcessLoopbackSource semantics.
//!
//! Intended Win32 API path:
//! - ActivateAudioInterfaceAsync
//! - AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK
//! - AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS
//! - PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE
//! - IAudioClient / IAudioCaptureClient
//! - event-driven shared mode
//!
//! Reference:
//! reference/le-audio-relay/src/Routing/ProcessLoopbackSource.cs
//!
//! This file is a compileable contract placeholder until First Sound is green.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessLoopbackMode {
    ExcludeCurrentProcessTree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessLoopbackConfig {
    pub mode: ProcessLoopbackMode,
    pub buffer_ms: u32,
}

impl Default for ProcessLoopbackConfig {
    fn default() -> Self {
        Self {
            mode: ProcessLoopbackMode::ExcludeCurrentProcessTree,
            buffer_ms: 10,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureTimestamp {
    pub device_position: u64,
    pub qpc_position: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureFlags {
    pub silent: bool,
}
