# R2 implementation pseudocode

## be200-runtime

```text
open():
    verify WinUSB binding
    claim BE200 interface 0
    read Intel version / current mode

    if mode == BOOTLOADER:
        require explicit firmware-boot action
        run already-validated R1.5b loader
        verify OPERATIONAL

    if mode == OPERATIONAL:
        attach without power-cycle

    construct Bumble transport/host adapter
    initialize Host capabilities WITHOUT first issuing HCI Reset
    return Runtime
```

Initial acceptance:
- same BE200 remains Operational;
- no firmware reload;
- Bumble side receives asynchronous HCI events;
- clean release.

## bap-client

```text
discover(peer):
    discover PACS
    discover ASCS
    read sink PAC records
    read sink ASE state
    ask device-policy for a StreamPlan

configure(plan):
    ASCS CONFIG_CODEC
    await Codec Configured
    allocate/configure CIG
    ASCS CONFIG_QOS
    await QoS Configured
    create CIS
    await CIS Established
    setup Host->Controller ISO data path
    ASCS ENABLE
    perform receiver-start-ready/start transition required by peer
    await STREAMING

stop():
    stop media scheduling
    ASCS DISABLE / RELEASE as state requires
    remove ISO data path
    disconnect/remove CIS/CIG in bounded order
```

Happy path follows Zephyr. Failure transitions follow AOSP.

## device-policy

```text
select(pacs, ases):
    prefer 48 kHz
    prefer 10 ms first-sound mode
    choose topology only from peer-advertised capability
    never infer left/right topology from device name
    return explicit StreamPlan

on_partial_failure(snapshot):
    decide whether to rebuild CIS only, reconfigure ASE, or reconnect ACL
```

No Buds3 Pro-specific topology is frozen until PACS/ASCS evidence is collected.

## iso-scheduler

```text
every negotiated SDU interval:
    obtain exactly one PCM quantum
    apply bounded drift policy
    encode LC3
    submit one SDU per active stream
    obey controller flow/backpressure
    record underrun/overrun/late-frame telemetry

never:
    use an unbounded queue
    silently increase latency to absorb clock drift
```

## audio-capture-win32

Target API path, implemented in Rust:

```text
ActivateAudioInterfaceAsync
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS
    exclude current process tree
        |
        v
IAudioClient / event-driven shared mode
        |
        v
IAudioCaptureClient
        |
        v
48 kHz Float32 stereo PCM
```

Behavioral oracle: `reference/le-audio-relay/Routing/ProcessLoopbackSource.cs`.

Port these semantics, not the .NET runtime:

- exclude the current process tree;
- shared event-driven capture;
- 48 kHz Float32 stereo;
- MMCSS `Pro Audio` intent;
- propagate silence flags and device/QPC positions.

First-sound mode bypasses this crate and uses a fixed WAV/sine source.

## audio-bridge

```text
audio-capture-win32
    -> 48 kHz Float32 stereo
    -> channel mapping from StreamPlan
    -> LC3 encoder input
```

## recovery-supervisor

```text
states:
    Waiting
    Connecting
    Bonding
    Discovering
    Configuring
    Streaming
    Recovering

ACL lost:
    tear down stream ownership and reconnect with bounded backoff

CIS lost but ACL alive:
    inspect ASE/CIG state
    attempt the smallest AOSP-supported recovery
    escalate to stream reconfigure, then ACL reconnect

never:
    loop indefinitely without backoff
    reuse stale ephemeral advertiser identity as bonded identity
```
