# First Sound HIL Profile

Status: **canonical / frozen test fixture**

This file defines the default First Sound signal and LE Audio transport
parameters used by current-path HIL. If a test intentionally overrides one of
these values, the override must be explicit in the command or test case.

## Signal

```text
sample rate        48,000 Hz
frame duration     10 ms
total duration     2,000 ms
frame count        200

left tone          400 Hz
right tone         600 Hz

digital peak       approximately -40 dBFS
PCM peak           328 / 32767
fade-in            100 ms cosine
fade-out           100 ms cosine
```

Channel identity is fixed:

```text
400 Hz = Left
600 Hz = Right
```

The dBFS value is a digital signal level, not calibrated acoustic SPL.

## LE Audio transport

```text
codec              LC3
SDU                 100 bytes / ear / frame
CIG                 1
left CIS            1
right CIS           2
PHY                 2M
RTN                 13
max transport lat.  95 ms
presentation delay  40 ms
```

## Logical stream identity

```text
SessionId           7
StreamId            11
```

## Default current-path HIL

The Round8 current-path First Sound HIL defaults to this 2-second / 200-frame
fixture when `--frames` is omitted.

Canonical command entry point:

```text
be200hub round8 control-first-sound ...
```
