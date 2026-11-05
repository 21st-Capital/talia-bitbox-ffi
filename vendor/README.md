# vendor/

## bitbox-api

`bitbox-api/` is bitbox-api **0.13.0 exactly as crates.io published it**, plus
only the patches in `patches/`. Cargo takes it from here through the
`[patch.crates-io]` section of `../Cargo.toml`.

- Published package: https://static.crates.io/crates/bitbox-api/bitbox-api-0.13.0.crate
- Its SHA-256: `d6fbc54fa856e199b598cea738c9eeb2d8a3c891617e3bef3a970c2374c29baf`
  (the checksum `Cargo.lock` pinned while the crate came from crates.io).
  The package was cut from upstream commit `fbaea302dce8363009a46dd85ed94045be917949`
  (tag `v0.13.0`, see `bitbox-api/.cargo_vcs_info.json`).
- Nothing in `bitbox-api/` is edited by hand. `.cargo-ok`, which cargo writes
  when it unpacks a crate, is not part of the package and is not here.

Check the folder at any time (needs the network, changes nothing):

    vendor/verify-bitbox-api.sh

It downloads the published package, checks its SHA-256, applies `patches/`
in name order and compares the result with `bitbox-api/` file by file.

### Patches

Each one is an upstream BitBox commit, exported with
`git format-patch -1 --full-index <commit> -- src` from
https://github.com/BitBoxSwiss/bitbox-api-rs and applied unchanged. Only its
`src/` part is taken: release bookkeeping (version bump, CHANGELOG,
bitbox-api's own Cargo.lock) stays out, so the package still says 0.13.0.

- `0001-antiklepto-validate-ECDSA-signatures-c0773a32.patch`: upstream
  `c0773a323e17bf21ef22137de129990ac6f3b498` ("antiklepto: validate ECDSA
  signatures", 2026-08-06, on upstream master; it is the fix upstream prepared
  as 0.13.1, which crates.io does not have yet). Before it, the anti-klepto
  check compared only the first 32 bytes of a device signature (and panicked
  on a shorter one). The second half was never checked, so a high-S value (the
  malleated twin of the real signature, one bit the device chooses on every
  signature) or a zero went into the PSBT. Now every ECDSA signature the
  device returns must be exactly 64 bytes (65 with a recovery id of 0 to 3 for
  a signed message), with both halves non-zero and in range and the second
  half low-S, before it is accepted. Schnorr (taproot) signatures are
  unaffected: the device protocol has no anti-klepto for them.
  Upstream commits since 0.13.0 that were NOT taken: persisted-pairing file
  permissions (5009eb5, for PersistedNoiseConfig, which bitbox-ffi does not
  use: the app keeps pairing keys in the Keychain), shared PSBT test vectors
  plus device-owned outputs from another account (d865ab5), removal of the
  TypeScript/WASM package (163f8d9) and "reset sessions on connect" (b9ec5ae,
  firmware 9.28 and later). None of them checks signatures, and each would
  need its own device test.

### When to drop this

Once a crates.io release of bitbox-api carries every patch in `patches/`,
point `Cargo.toml` back at crates.io, delete the `[patch.crates-io]` section
and this folder, and run `cargo update -p bitbox-api`.
