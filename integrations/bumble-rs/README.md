# Bumble-RS integration seam

Pinned audited upstream revision:

`bbac2a6803b8cab0920ab725a23aa408fc4fed85`

Target crates:

- bumble-transport
- bumble-hci
- bumble-host
- bumble-gatt
- bumble-smp
- bumble-profiles
- bumble-codecs

## Required local adaptation before hardware connection

Bumble-RS's normal host power-on path sends HCI Reset. The first BE200 R2
experiment must instead attach to the already Operational controller without
silently resetting it.

The desired patch seam is approximately:

```text
Device::power_on(...)
    |
    +-- standard upstream path -> HCI Reset + normal initialization
    |
    +-- BE200 attach path ------> preserve operational controller,
                                  initialize host-side state,
                                  read capabilities,
                                  install event masks/features,
                                  no first Reset
```

Do not fork more code than necessary. Prefer an upstreamable option or a small
adapter/patch with tests.
