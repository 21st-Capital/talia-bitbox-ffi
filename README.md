# bitbox-ffi

This repository,
[21st-Capital/talia-bitbox-ffi](https://github.com/21st-Capital/talia-bitbox-ffi),
holds the Rust bridge between the Talia iOS app
([21st-Capital/talia](https://github.com/21st-Capital/talia)) and the BitBox02
Nova hardware wallet. It wraps Shift Crypto's
[bitbox-api](https://github.com/BitBoxSwiss/bitbox-api-rs) crate in a
[uniffi](https://github.com/mozilla/uniffi-rs) interface that the app calls
from Swift, and builds as a static library, `libbitboxffi.a`, for iOS devices
and the iOS simulator.

## This copy

The repository is published as a fresh copy of a reviewed commit of 21st
Capital's private repository: its files are exactly that commit's, without
the version history. When Talia ships an App Store release, a script exports
the reviewed commit, scans it and adds it here as one new commit.

Issues are welcome. Pull requests from outside 21st Capital are not accepted
for now: a change is made in the private repository and arrives here with the
next copy.

## What it does

No BitBox SDK ships a Bluetooth transport, so the work is split:

- The app owns the radio: scanning, connecting, the MTU, and cutting the
  stream into 64-byte frames.
- This crate owns the protocol: U2F-HID command framing (bitbox-api adds it),
  the Noise encrypted channel, pairing, and the Bitcoin calls.

The seam is small. Rust hands every 64-byte frame it wants sent to a
`FrameSink` callback that the app implements, and the app passes each frame
the device sends back to `feed_frame`. Nothing about Bluetooth exists on the
Rust side.

The interface is [`src/bitbox.udl`](src/bitbox.udl) and the code is
[`src/lib.rs`](src/lib.rs). On top of bitbox-api, the bridge adds:

- **A handle registry.** `create_client` registers a client and returns a
  handle without any I/O; `open` then makes first contact. They are separate
  on purpose: the device's first answer arrives through `feed_frame`, which
  needs the handle.
- **`GentleRuntime`**, which sleeps exactly as long as bitbox-api asks. The
  device expects a poll every 200 ms and drops a pending request when a poll
  is late, so the bridge never lengthens the poll interval.
- **An in-memory Noise store.** bitbox-api saves the pairing keys
  synchronously during the handshake. The app reads them afterwards with
  `noise_config_json`, keeps them in the iOS Keychain and passes them back to
  `create_client` next time, so the next pairing needs no code on the screen.
- **Pairing and device calls:** `unlock_and_pair`, `confirm_pairing`,
  `device_info`, `root_fingerprint` and `xpub`.
- **Multisig wallets:** `is_policy_registered`, `register_policy` and
  `policy_address`. A `wsh(sortedmulti(...))` wallet goes to the device as its
  classic multisig configuration (sorted keys give the same script), because
  the firmware's policy parser does not accept `sortedmulti`. Every other
  wallet goes as a BIP-388 wallet policy.
- **Signing:** `sign_psbt`, for multisig and single-sig wallets alike. For a
  multisig wallet the caller passes the policy that was registered; for a
  single-sig wallet it passes none, and the device reads the PSBT's own key
  origins.

Every exported call runs synchronously on the caller's thread
(`futures::executor::block_on`), so the app calls `feed_frame` and the
operations from different threads. The device signs but does not finalize a
PSBT; the app does that.

## Upstream code

| Component   | Version                                       | Taken from                            |
|-------------|-----------------------------------------------|---------------------------------------|
| bitbox-api  | 0.13.0 plus upstream commit `c0773a32`        | vendored in `vendor/bitbox-api`       |
| uniffi      | 0.23.0                                        | crates.io, pinned by `Cargo.lock`     |
| bitcoin     | 0.32 (rust-bitcoin)                           | crates.io, pinned by `Cargo.lock`     |

`vendor/bitbox-api` is bitbox-api 0.13.0 exactly as crates.io published it
(package SHA-256
`d6fbc54fa856e199b598cea738c9eeb2d8a3c891617e3bef3a970c2374c29baf`, cut from
upstream commit `fbaea302dce8363009a46dd85ed94045be917949`, tag `v0.13.0`),
plus one upstream patch from `vendor/patches`: commit
`c0773a323e17bf21ef22137de129990ac6f3b498`, "antiklepto: validate ECDSA
signatures", the fix upstream prepared as 0.13.1. Cargo takes the folder
through the `[patch.crates-io]` section of `Cargo.toml`.
[`vendor/README.md`](vendor/README.md) explains the patch, the upstream
commits that were not taken, and when the folder can go.

To check the vendored folder (it needs the network and changes nothing):

    vendor/verify-bitbox-api.sh

It downloads the published package, checks its SHA-256, applies
`vendor/patches` in name order and compares the result with
`vendor/bitbox-api` file by file.

`vendor/bitbox-api/tests` holds upstream's tests, including the public seed
and keys of the BitBox simulator. They are published test values, not
secrets.

## Toolchain

- **Rust 1.97.1**, pinned in `rust-toolchain.toml` together with the targets
  `aarch64-apple-ios`, `aarch64-apple-ios-sim` and `x86_64-apple-ios`. rustup
  installs them on the first build.
- **iOS 15.0**: `.cargo/config.toml` sets `IPHONEOS_DEPLOYMENT_TARGET` for the
  Rust and the C code alike, and a shell variable cannot override it.
- **Xcode**: the secp256k1 C library inside (four objects in each archive) is
  compiled by the installed Xcode's clang against its iOS SDK, and each of
  those four objects records the SDK version. Another Xcode gives other bytes.
  The archive's other objects that are not compiled from Rust (compiler-rt's,
  part of Rust's `compiler_builtins`) come prebuilt with the Rust toolchain and
  do not depend on Xcode.

## Build

    cargo build --locked --release --target aarch64-apple-ios
    cargo build --locked --release --target aarch64-apple-ios-sim
    cargo run --locked --release --bin uniffi-bindgen -- \
      generate src/bitbox.udl --language swift --out-dir generated

This writes `target/aarch64-apple-ios/release/libbitboxffi.a`,
`target/aarch64-apple-ios-sim/release/libbitboxffi.a`,
`generated/bitbox.swift`, `generated/bitboxFFI.h` and
`generated/bitboxFFI.modulemap`. The app takes the two libraries,
`bitbox.swift` and `bitboxFFI.h`.

`cargo test --locked` runs the unit tests in `src/lib.rs` on the build
machine.

## Rebuild and compare with the app

The Talia app builds this crate with its `native/build-bitboxffi.sh` and
records the SHA-256 of both libraries in its `native/native-libs.sha256`, in
the `# built bitboxffi` lines, next to a `# source bitboxffi` line that names
the commit. The script:

- runs cargo under `env -i` with only `HOME`, `PATH`, `USER`, `LOGNAME`,
  `SHELL`, `TMPDIR`, `LANG`, `TERM`, `SSH_AUTH_SOCK`, `LC_*` and the proxy
  variables, so no `RUSTFLAGS`, `CARGO_*`, `CC` or `CFLAGS` reaches the build;
- uses `target` inside the checkout as the target folder;
- runs the three commands above, each with `--target-dir <checkout>/target`.

To compare a build of your own:

1. Check out the commit and note its tree hash, `git rev-parse 'HEAD^{tree}'`,
   which depends only on the files, not on the commit's history.
2. Run `vendor/verify-bitbox-api.sh`.
3. Build as above, in the same reduced environment, for example
   `env -i HOME="$HOME" PATH="$PATH" cargo build --locked --target-dir "$PWD/target" --release --target aarch64-apple-ios`.
4. Run `shasum -a 256` on both `libbitboxffi.a` files and compare with the
   `# built bitboxffi` lines.

Two builds of the same commit, at the same path, with the same Rust and the
same Xcode, give identical bytes. The digests match the app's record only
when all of these are the same as for the recorded build:

- the commit, with its `Cargo.lock`;
- rustc 1.97.1, installed by rustup in the same folder;
- the Xcode, because of its clang and iOS SDK;
- the absolute path of the checkout: the uniffi scaffolding is included from
  the build's `OUT_DIR`, and that absolute path is compiled into panic
  locations;
- the absolute path of `CARGO_HOME`, because the source folder of every
  dependency in the cargo registry is compiled into panic locations too.

A build at another path, or with another Xcode, gives other bytes from the
same source. For example, a rebuild of the same commit at another checkout
path, with a newer Xcode and the same `CARGO_HOME`, changed only the crate's
own two objects that carry the checkout path, the archive's symbol table, and
one byte (the SDK version) in each of the four secp256k1 objects; every other
member of the archive stayed byte-identical. Making the bytes independent of
the paths (remapping the path prefixes, and keeping the uniffi bindgen code
that the default `uniffi/cli` feature compiles in out of the library) changes
the library, so it is left for a change of its own.

## Security

Please report a vulnerability privately, through GitHub's **Report a
vulnerability** on this repository, never in a public issue or pull request.
[`SECURITY.md`](SECURITY.md) says how, and where problems in upstream code
belong.

## Licence

Apache-2.0, see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE). `vendor/bitbox-api` keeps its own
Apache-2.0 licence file, `vendor/bitbox-api/LICENSE` (Shift Crypto AG). The
dependencies taken from crates.io carry their own licences; uniffi is
MPL-2.0.
