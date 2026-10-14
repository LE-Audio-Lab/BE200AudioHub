# Third-Party Notices

BE200AudioHub is licensed under the Apache License, Version 2.0. The project
also depends on third-party software that remains subject to its own license.

This file is an attribution and distribution aid. It does not replace the
license text shipped by any third-party project.

## Direct dependencies

### bumble-rs

Repository: https://github.com/chaitanyarahalkar/bumble-rs

License: Apache-2.0

Copyright 2026 The bumble-rs contributors.

bumble-rs is an independent Rust port of parts of Google's Bumble Bluetooth
stack. Its own NOTICE file states that wire formats, constants, and
cryptographic test vectors are derived from Google Bumble and the Bluetooth
Core Specification.

### Google Bumble

Repository: https://github.com/google/bumble

License: Apache-2.0

Google Bumble is not a direct Cargo dependency of BE200AudioHub, but it is an
upstream implementation and provenance source for bumble-rs and is referenced
throughout the project's engineering documentation and compatibility work.

### rusb

Repository: https://github.com/a1ien/rusb

License: MIT

BE200AudioHub enables rusb's `vendored` feature.

### libusb

Repository: https://github.com/libusb/libusb

License: LGPL-2.1-or-later

The rusb vendored build can compile and link libusb into distributed binaries.
The Apache-2.0 license for BE200AudioHub does not replace or weaken libusb's
LGPL terms. Anyone distributing binaries that contain or link libusb is
responsible for satisfying the applicable LGPL source, notice, and relinking
requirements.

### crossbeam-channel

Repository: https://github.com/crossbeam-rs/crossbeam

License: MIT OR Apache-2.0

### sha2 / RustCrypto hashes

Repository: https://github.com/RustCrypto/hashes

License: MIT OR Apache-2.0

### serde_json

Repository: https://github.com/serde-rs/json

License: MIT OR Apache-2.0

## Firmware and hardware-vendor material

Intel firmware images, including `.sfi` and `.ddc` blobs used during BE200
bootstrap or HIL, are third-party material and are **not** licensed under the
BE200AudioHub Apache-2.0 license.

The source repository should contain loader code, expected metadata, hashes,
and compatibility information rather than representing third-party firmware as
project-owned Apache-2.0 content. Redistribution of any firmware image must be
handled according to the license and distribution terms supplied by its
copyright owner.

## Bluetooth specifications

Bluetooth specifications, assigned numbers, trademarks, and related material
remain the property of their respective rights holders. References to Bluetooth
SIG specifications or assigned values in this repository do not grant any
additional rights to those materials or marks.
