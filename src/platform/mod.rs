//! L3 Platform / Device Backend.
//!
//! Owns OS/hardware-specific raw I/O, USB primitives, Intel firmware bootstrap,
//! and deterministic backend implementations. Bluetooth semantics stay above L3.

pub mod backend;
pub mod firmware;
pub mod usb;
pub mod windows;
