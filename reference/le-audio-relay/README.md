# LE-Audio-Relay reference snapshot

This directory is **reference-only** and is excluded from the BE200AudioHub build graph.

Canonical source:

`STanJK/LE-Audio-Relay @ 3f75ae1a487bf92aaaba8dfe37fac36ab6576f59`

The C# files are preserved verbatim as behavioral oracles for the Rust port:

- Windows Process Loopback capture;
- 48 kHz Float32 stereo format policy;
- bounded SPSC PCM ring;
- startup cushion;
- provisional positive-drift guard;
- telemetry;
- worker/supervisor vocabulary.

Target Rust ownership:

- `crates/audio-capture-win32`
- `crates/audio-bridge`
- `crates/iso-scheduler`
- `crates/recovery-supervisor`

There is intentionally no `.csproj` here. The final product is not a Rust/.NET mixed-runtime application.
