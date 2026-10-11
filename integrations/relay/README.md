# LE-Audio-Relay integration seam

Canonical source: STanJK/LE-Audio-Relay @
`3f75ae1a487bf92aaaba8dfe37fac36ab6576f59`.

Do not duplicate the entire .NET application here.

First integration reuses these components in their existing .NET process:

- Routing/ProcessLoopbackSource.cs
- Routing/AudioFormatPolicy.cs
- Timing/SpscPcmRing.cs
- Timing/PcmRelayBoundary.cs
- Timing/ProvisionalPositiveDriftGuard.cs
- Supervision/WorkerProtocol.cs
- Supervision/SupervisorState.cs

Boundary contract:

```text
48 kHz / Float32 / stereo PCM
bounded ring + startup cushion
          |
          v
versioned local IPC
          |
          v
Rust audio-bridge / iso-scheduler
```

Why IPC first instead of a Rust WASAPI rewrite:

1. the Relay path is already daily-use validated;
2. first-sound debugging must not combine Windows audio and Bluetooth Host failures;
3. the backend-worker isolation model remains useful if the controller or host stack wedges.

The concrete IPC transport is intentionally not selected in this bootstrap commit.
