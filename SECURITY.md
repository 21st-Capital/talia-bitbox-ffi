# Security Policy

This policy covers
[21st-Capital/talia-bitbox-ffi](https://github.com/21st-Capital/talia-bitbox-ffi),
the Rust bridge between the Talia app and the BitBox02 Nova: the bridge's own
code (`src/`, `build.rs` and the build settings) and the way it vendors
bitbox-api (`vendor/`: the crates.io release plus one upstream patch). The
library built from it is part of the Talia wallet app; it is not published as
a crate of its own.

## Reporting a vulnerability

Please report a vulnerability privately, never in a public issue or pull
request. Use GitHub's private vulnerability reporting: on this repository's
**Security** tab, choose **Report a vulnerability**, or open
<https://github.com/21st-Capital/talia-bitbox-ffi/security/advisories/new>.
The report stays private between you and the repository's maintainers.

Say which commit you looked at, what an attacker can do, and how to reproduce
it.

## Upstream code

A problem in code this repository does not change belongs to the upstream
project, which handles security reports its own way:

- bitbox-api: [BitBoxSwiss/bitbox-api-rs](https://github.com/BitBoxSwiss/bitbox-api-rs)
- the BitBox02 firmware: [BitBoxSwiss/bitbox02-firmware](https://github.com/BitBoxSwiss/bitbox02-firmware)
- uniffi: [mozilla/uniffi-rs](https://github.com/mozilla/uniffi-rs)
- rust-bitcoin: [rust-bitcoin/rust-bitcoin](https://github.com/rust-bitcoin/rust-bitcoin)

If you are not sure whether a problem is upstream's or this repository's,
report it to us and we will pass it on.
