//! Talia's bridge to `bitbox-api`: the BitBox02 Nova, over a Bluetooth link
//! the APP owns.
//!
//! No BitBox SDK ships a BLE transport, so the split is:
//!
//!   * JS (react-native-ble-plx) owns the radio: scan, connect, MTU, the
//!     64-byte framing and the 5-frame write batching (proven on a BitBox02
//!     Nova on 2026-08-31 -- see src/bitcoin/bitbox/ in the Talia app).
//!   * This crate owns the PROTOCOL: U2F-HID command framing (bitbox-api adds
//!     it in from_transport), the Noise 'Whisper' encryption, pairing, and
//!     the BTC calls.
//!
//! The seam is deliberately dumb: Rust hands 64-byte frames to a FrameSink
//! callback, and the app feeds device frames back with feed_frame. Nothing
//! about Bluetooth exists on this side of the boundary.
//!
//! CONCURRENCY MODEL. Every exported op runs the whole future to completion
//! with `futures::executor::block_on` on the CALLER's thread (DefaultRuntime
//! sleeps with std::thread::sleep; its docs pair it with exactly this).
//! feed_frame and the ops must therefore arrive on DIFFERENT threads -- the
//! native module dispatches ops on one queue and frames on another. The
//! FrameSink callback fires on the blocked op thread: implementations must
//! queue and return, never wait.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::executor::block_on;

/// Sleeps EXACTLY as asked. The tempting improvement is written here so it is
/// never tried again: flooring the NOTREADY poll at 1s (to stop the password
/// entry stuttering under the 200ms poll traffic) made the firmware NACK the
/// retry, because the device holds an outstanding request only briefly
/// between polls and DROPS it when the next poll is late. Testing tied the
/// regression to that exact build, and the vendor's own Go client confirms
/// the contract: it polls at precisely 200ms and logs the retry intervals
/// whenever it sees an unexpected NACK, meaning slow polls are a known
/// failure mode (bitbox02-api-go query.go, read 2026-08-31). The poll
/// cadence is protocol, not politeness.
pub struct GentleRuntime;

#[async_trait::async_trait]
impl bitbox_api::runtime::Runtime for GentleRuntime {
    async fn sleep(dur: std::time::Duration) {
        std::thread::sleep(dur);
    }
}

/// One name so every state and call agrees on the runtime.
type Rt = GentleRuntime;

uniffi::include_scaffolding!("bitbox");

// ---- Errors -----------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum BitBoxError {
    #[error("{message}")]
    Device { message: String },
    #[error("{message}")]
    State { message: String },
}

fn device_err(e: impl std::fmt::Display) -> BitBoxError {
    BitBoxError::Device { message: e.to_string() }
}
fn state_err(message: &str) -> BitBoxError {
    BitBoxError::State { message: message.to_string() }
}

// ---- The transport ----------------------------------------------------------

pub trait FrameSink: Send + Sync {
    fn send(&self, frame: Vec<u8>);
}

/// ReadWrite over the app's BLE link. `write` hands the frame to the app and
/// returns; `read` blocks on the channel the app feeds. The device answers a
/// query with silence only when something is truly wrong, so the read timeout
/// is generous: user confirmations happen on the DEVICE and legitimately take
/// as long as a human takes.
struct BleTransport {
    sink: Box<dyn FrameSink>,
    incoming: Mutex<mpsc::Receiver<Vec<u8>>>,
}

impl bitbox_api::Threading for BleTransport {}

#[async_trait::async_trait]
impl bitbox_api::ReadWrite for BleTransport {
    fn write(&self, msg: &[u8]) -> Result<usize, bitbox_api::CommunicationError> {
        self.sink.send(msg.to_vec());
        Ok(msg.len())
    }

    async fn read(&self) -> Result<Vec<u8>, bitbox_api::CommunicationError> {
        // Block_on runs this future alone on the op thread, so a blocking
        // recv is honest here: there is nothing else to poll.
        let rx = self.incoming.lock().unwrap();
        rx.recv_timeout(Duration::from_secs(180))
            .map_err(|_| bitbox_api::CommunicationError::Read)
    }
}

// ---- Noise persistence ------------------------------------------------------

/// bitbox-api calls store_config synchronously, twice, mid-handshake. iOS
/// Keychain access is async from JS, so persistence is split: this holds the
/// config in memory during the session, and the app reads the final state
/// with noise_config_json AFTER pairing and writes it to the Keychain itself.
#[derive(Default)]
struct MemNoise {
    data: Mutex<bitbox_api::NoiseConfigData>,
}

impl bitbox_api::Threading for MemNoise {}

impl bitbox_api::NoiseConfig for MemNoise {
    fn read_config(&self) -> Result<bitbox_api::NoiseConfigData, bitbox_api::ConfigError> {
        let d = self.data.lock().unwrap();
        Ok(bitbox_api::NoiseConfigData {
            app_static_privkey: d.app_static_privkey,
            device_static_pubkeys: d.device_static_pubkeys.clone(),
        })
    }
    fn store_config(&self, conf: &bitbox_api::NoiseConfigData) -> Result<(), bitbox_api::ConfigError> {
        let mut d = self.data.lock().unwrap();
        d.app_static_privkey = conf.app_static_privkey;
        d.device_static_pubkeys = conf.device_static_pubkeys.clone();
        Ok(())
    }
}

/// The wire format the app persists. Hex, not raw: it goes through JSON, a
/// JS string, and the Keychain.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct NoiseJson {
    app_static_privkey_hex: Option<String>,
    device_static_pubkeys_hex: Vec<String>,
}

fn noise_from_json(json: &str) -> Result<bitbox_api::NoiseConfigData, BitBoxError> {
    let parsed: NoiseJson = serde_json::from_str(json)
        .map_err(|e| state_err(&format!("Bad noise config: {e}")))?;
    let privkey = match parsed.app_static_privkey_hex {
        None => None,
        Some(h) => {
            let bytes = hex::decode(&h).map_err(|e| state_err(&format!("Bad noise key: {e}")))?;
            let arr: [u8; 32] = bytes
                .try_into()
                .map_err(|_| state_err("Noise key is not 32 bytes"))?;
            Some(arr)
        }
    };
    let pubkeys = parsed
        .device_static_pubkeys_hex
        .iter()
        .map(|h| hex::decode(h).map_err(|e| state_err(&format!("Bad device key: {e}"))))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(bitbox_api::NoiseConfigData {
        app_static_privkey: privkey,
        device_static_pubkeys: pubkeys,
    })
}

fn noise_to_json(d: &bitbox_api::NoiseConfigData) -> String {
    serde_json::to_string(&NoiseJson {
        app_static_privkey_hex: d.app_static_privkey.map(hex::encode),
        device_static_pubkeys_hex: d.device_static_pubkeys.iter().map(hex::encode).collect(),
    })
    .expect("static shape always serializes")
}

// ---- The state machine ------------------------------------------------------

pub struct DeviceInfo {
    pub name: String,
    pub version: String,
    pub initialized: bool,
    pub mnemonic_passphrase_enabled: bool,
}

pub enum XPubType {
    Tpub,
    Xpub,
    Zpub,
    Vpub,
}

/// bitbox-api's states are CONSUMING (unlock_and_pair takes self), so the
/// registry holds an enum and ops move the value through it. `Busy` marks a
/// client currently inside an op; a second op on the same handle is a caller
/// bug and is refused rather than deadlocked.
enum ClientState {
    /// Registered but not yet talked to. create_client MUST NOT do I/O: the
    /// device's first response arrives through feed_frame, which needs the
    /// handle, which create_client has not returned yet. The first on-device
    /// run deadlocked on exactly that circle (test 2026-08-31: "sent 1 frame,
    /// received 1", and the received frame sat in a JS buffer waiting for
    /// the very call it would have completed). open() does the talking.
    Unopened {
        sink: Box<dyn FrameSink>,
        rx: mpsc::Receiver<Vec<u8>>,
    },
    Fresh(bitbox_api::BitBox<Rt>),
    Pairing(bitbox_api::PairingBitBox<Rt>),
    Paired(bitbox_api::PairedBitBox<Rt>),
    Busy,
    /// Creation or a consuming op failed; the client is gone for good. Kept
    /// so later calls say WHY instead of the misleading "already busy".
    Dead,
}

struct Entry {
    state: ClientState,
    noise: Arc<MemNoise>,
    feeder: mpsc::Sender<Vec<u8>>,
}

static REGISTRY: Mutex<Option<HashMap<u64, Entry>>> = Mutex::new(None);
static NEXT_HANDLE: Mutex<u64> = Mutex::new(1);

fn with_registry<T>(f: impl FnOnce(&mut HashMap<u64, Entry>) -> T) -> T {
    let mut guard = REGISTRY.lock().unwrap();
    f(guard.get_or_insert_with(HashMap::new))
}

// ---- Exported operations ----------------------------------------------------

/// Registers the client and returns at once. NO I/O happens here; call
/// open() next. See ClientState::Unopened for the deadlock this prevents.
pub fn create_client(sink: Box<dyn FrameSink>, noise_config_json: Option<String>) -> u64 {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let noise = Arc::new(MemNoise::default());
    if let Some(json) = noise_config_json {
        if let Ok(data) = noise_from_json(&json) {
            let _ = bitbox_api::NoiseConfig::store_config(&*noise, &data);
        }
        // A corrupt stored config is not fatal: pairing simply starts fresh,
        // which is the honest recovery for a Keychain blob gone bad.
    }
    let handle = {
        let mut n = NEXT_HANDLE.lock().unwrap();
        let h = *n;
        *n += 1;
        h
    };
    with_registry(|r| {
        r.insert(
            handle,
            Entry { state: ClientState::Unopened { sink, rx }, noise, feeder: tx },
        );
    });
    handle
}

/// First contact: the protocol-info query runs here, with the frame pump
/// already able to route the answer to this handle.
pub fn open(handle: u64) -> Result<(), BitBoxError> {
    match take_state(handle)? {
        ClientState::Unopened { sink, rx } => {
            let noise = with_registry(|r| r.get(&handle).map(|e| e.noise.clone()))
                .ok_or_else(|| state_err("Unknown device handle."))?;
            let transport = BleTransport { sink, incoming: Mutex::new(rx) };
            match block_on(bitbox_api::BitBox::<Rt>::from_transport(
                Box::new(transport),
                Box::new(NoiseHandle(noise)),
            )) {
                Ok(c) => {
                    put_state(handle, ClientState::Fresh(c));
                    Ok(())
                }
                Err(e) => {
                    put_state(handle, ClientState::Dead);
                    Err(device_err(e))
                }
            }
        }
        other => {
            put_state(handle, other);
            Err(state_err("This device is already open."))
        }
    }
}

/// NoiseConfig is passed by Box, but the registry must keep reading the same
/// storage the client writes to. This forwards to the shared Arc.
struct NoiseHandle(Arc<MemNoise>);
impl bitbox_api::Threading for NoiseHandle {}
impl bitbox_api::NoiseConfig for NoiseHandle {
    fn read_config(&self) -> Result<bitbox_api::NoiseConfigData, bitbox_api::ConfigError> {
        bitbox_api::NoiseConfig::read_config(&*self.0)
    }
    fn store_config(&self, conf: &bitbox_api::NoiseConfigData) -> Result<(), bitbox_api::ConfigError> {
        bitbox_api::NoiseConfig::store_config(&*self.0, conf)
    }
}

pub fn feed_frame(handle: u64, frame: Vec<u8>) {
    let feeder = with_registry(|r| r.get(&handle).map(|e| e.feeder.clone()));
    if let Some(tx) = feeder {
        let _ = tx.send(frame); // a closed client just drops late frames
    }
}

/// Take the state out for an op, leaving Busy in its place.
fn take_state(handle: u64) -> Result<ClientState, BitBoxError> {
    with_registry(|r| {
        let entry = r.get_mut(&handle).ok_or_else(|| state_err("Unknown device handle."))?;
        match std::mem::replace(&mut entry.state, ClientState::Busy) {
            ClientState::Busy => Err(state_err("The device is already busy with another call.")),
            ClientState::Dead => {
                entry.state = ClientState::Dead;
                Err(state_err(
                    "The connection to the BitBox failed while it was being set up. Go back and reconnect.",
                ))
            }
            other => Ok(other),
        }
    })
}

fn put_state(handle: u64, state: ClientState) {
    with_registry(|r| {
        if let Some(entry) = r.get_mut(&handle) {
            entry.state = state;
        }
    });
}

pub fn unlock_and_pair(handle: u64) -> Result<Option<String>, BitBoxError> {
    match take_state(handle)? {
        ClientState::Fresh(bb) => match block_on(bb.unlock_and_pair()) {
            Ok(pairing) => {
                let code = pairing.get_pairing_code();
                put_state(handle, ClientState::Pairing(pairing));
                Ok(code)
            }
            Err(e) => {
                put_state(handle, ClientState::Dead);
                Err(device_err(e))
            }
        },
        other => {
            put_state(handle, other);
            Err(state_err("This device is already past unlocking."))
        }
    }
}

pub fn confirm_pairing(handle: u64) -> Result<(), BitBoxError> {
    match take_state(handle)? {
        ClientState::Pairing(p) => match block_on(p.wait_confirm()) {
            Ok(paired) => {
                put_state(handle, ClientState::Paired(paired));
                Ok(())
            }
            Err(e) => {
                put_state(handle, ClientState::Dead);
                Err(device_err(e))
            }
        },
        other => {
            put_state(handle, other);
            Err(state_err("Pairing has not been started on this device."))
        }
    }
}

fn with_paired<T>(
    handle: u64,
    f: impl FnOnce(&bitbox_api::PairedBitBox<Rt>) -> Result<T, BitBoxError>,
) -> Result<T, BitBoxError> {
    // Paired ops take &self, so the state can go straight back before the op
    // result is inspected -- but it must be OUT while running so a concurrent
    // op is refused, hence take/put around the call.
    match take_state(handle)? {
        ClientState::Paired(p) => {
            let out = f(&p);
            put_state(handle, ClientState::Paired(p));
            out
        }
        other => {
            put_state(handle, other);
            Err(state_err("Pair with the device first."))
        }
    }
}

pub fn noise_config_json(handle: u64) -> Result<String, BitBoxError> {
    let noise = with_registry(|r| r.get(&handle).map(|e| e.noise.clone()))
        .ok_or_else(|| state_err("Unknown device handle."))?;
    let data = bitbox_api::NoiseConfig::read_config(&*noise)
        .map_err(|e| device_err(format!("{e:?}")))?;
    Ok(noise_to_json(&data))
}

pub fn device_info(handle: u64) -> Result<DeviceInfo, BitBoxError> {
    with_paired(handle, |p| {
        let info = block_on(p.device_info()).map_err(device_err)?;
        Ok(DeviceInfo {
            name: info.name,
            version: info.version,
            initialized: info.initialized,
            mnemonic_passphrase_enabled: info.mnemonic_passphrase_enabled,
        })
    })
}

pub fn root_fingerprint(handle: u64) -> Result<String, BitBoxError> {
    with_paired(handle, |p| block_on(p.root_fingerprint()).map_err(device_err))
}

pub fn xpub(
    handle: u64,
    keypath: String,
    xpub_type: XPubType,
    display: bool,
) -> Result<String, BitBoxError> {
    with_paired(handle, |p| {
        let kp = keypath
            .as_str()
            .try_into()
            .map_err(|e: <bitbox_api::Keypath as TryFrom<&str>>::Error| device_err(e))?;
        let t = match xpub_type {
            XPubType::Tpub => bitbox_api::pb::btc_pub_request::XPubType::Tpub,
            XPubType::Xpub => bitbox_api::pb::btc_pub_request::XPubType::Xpub,
            XPubType::Zpub => bitbox_api::pb::btc_pub_request::XPubType::Zpub,
            XPubType::Vpub => bitbox_api::pb::btc_pub_request::XPubType::Vpub,
        };
        block_on(p.btc_xpub(get_coin_for(&keypath), &kp, t, display)).map_err(device_err)
    })
}

/// The coin follows the keypath: 1' anywhere in the first two hardened
/// components means testnet. The device itself validates the full path, so
/// this only has to pick the request's coin enum.
fn get_coin_for(keypath: &str) -> bitbox_api::pb::BtcCoin {
    if keypath.contains("/1'") || keypath.contains("/1h") {
        bitbox_api::pb::BtcCoin::Tbtc
    } else {
        bitbox_api::pb::BtcCoin::Btc
    }
}

pub fn close(handle: u64) {
    with_registry(|r| {
        r.remove(&handle);
    });
}

// ---- Registration, address check, signing -----------------------------------

pub struct PolicyKey {
    pub root_fingerprint_hex: Option<String>,
    pub keypath: Option<String>,
    pub xpub: String,
}

fn coin_for(testnet: bool) -> bitbox_api::pb::BtcCoin {
    if testnet {
        bitbox_api::pb::BtcCoin::Tbtc
    } else {
        bitbox_api::pb::BtcCoin::Btc
    }
}

fn parse_keypath(s: &str) -> Result<bitbox_api::Keypath, BitBoxError> {
    s.try_into()
        .map_err(|e: <bitbox_api::Keypath as TryFrom<&str>>::Error| {
            state_err(&format!("Bad keypath {s}: {e}"))
        })
}

fn to_key_origin(k: &PolicyKey) -> Result<bitbox_api::btc::KeyOriginInfo, BitBoxError> {
    use std::str::FromStr;
    let xpub = bitcoin::bip32::Xpub::from_str(&k.xpub)
        .map_err(|e| state_err(&format!("Bad xpub: {e}")))?;
    let root_fingerprint = match &k.root_fingerprint_hex {
        None => None,
        Some(h) => Some(
            bitcoin::bip32::Fingerprint::from_str(h)
                .map_err(|e| state_err(&format!("Bad fingerprint {h}: {e}")))?,
        ),
    };
    let keypath = match &k.keypath {
        None => None,
        Some(p) => Some(parse_keypath(p)?),
    };
    Ok(bitbox_api::btc::KeyOriginInfo { root_fingerprint, keypath, xpub })
}

fn policy_config(
    policy: &str,
    keys: &[PolicyKey],
) -> Result<bitbox_api::pb::BtcScriptConfig, BitBoxError> {
    let origins = keys.iter().map(to_key_origin).collect::<Result<Vec<_>, _>>()?;
    Ok(bitbox_api::btc::make_script_config_policy(policy, &origins))
}

/// `wsh(sortedmulti(k,@0/**,...))` → (k, key index per slot), or None for
/// anything else.
///
/// A BitBox "policy" must be miniscript, and `sortedmulti` is not a
/// miniscript fragment: the firmware's parser (policies.rs) refuses
/// `wsh(sortedmulti(...))` with a bare "invalid input". That is exactly what
/// a plain self-custodial multisig is (wsh since 2026-07-28), so the BitBox
/// could join every Talia shape except the simplest. The device has a
/// separate classic MULTISIG config whose keys are BIP-67 sorted, i.e. the
/// same script as sortedmulti, so these vaults go through that instead. It
/// only knows receive/change 0 and 1, hence only `/**` or `/<0;1>/*`.
fn parse_wsh_sortedmulti(policy: &str) -> Option<(u32, Vec<usize>)> {
    let inner = policy.strip_prefix("wsh(sortedmulti(")?.strip_suffix("))")?;
    let mut parts = inner.split(',').map(str::trim);
    let threshold = parts.next()?.parse().ok()?;
    let order = parts
        .map(|k| {
            let k = k.strip_prefix('@')?;
            let n = k.strip_suffix("/**").or_else(|| k.strip_suffix("/<0;1>/*"))?;
            n.parse().ok()
        })
        .collect::<Option<Vec<usize>>>()?;
    Some((threshold, order))
}

/// The device's config for this vault: the classic multisig config for a
/// wsh(sortedmulti), a BIP-388 policy for everything else. `our_keypath` is
/// the account keypath, or a full address keypath under it.
fn script_config(
    p: &bitbox_api::PairedBitBox<Rt>,
    policy: &str,
    keys: &[PolicyKey],
    our_keypath: &bitbox_api::Keypath,
) -> Result<bitbox_api::pb::BtcScriptConfig, BitBoxError> {
    let Some((threshold, order)) = parse_wsh_sortedmulti(policy) else {
        return policy_config(policy, keys);
    };
    // The multisig config names our key by POSITION, so find it: our root
    // fingerprint, at an account path the requested keypath lives under.
    let fp = block_on(p.root_fingerprint()).map_err(device_err)?;
    let ours = our_keypath.to_vec();
    let mut xpubs = Vec::with_capacity(order.len());
    let mut our_index = None;
    for (slot, &i) in order.iter().enumerate() {
        let k = keys
            .get(i)
            .ok_or_else(|| state_err("The vault names a key it does not list."))?;
        let origin = to_key_origin(k)?;
        let is_ours = origin.root_fingerprint.map(|f| f.to_string()) == Some(fp.clone())
            && origin
                .keypath
                .as_ref()
                .is_some_and(|kp| ours.starts_with(&kp.to_vec()));
        if is_ours && our_index.is_none() {
            our_index = Some(slot as u32);
        }
        xpubs.push(origin.xpub);
    }
    let our_index =
        our_index.ok_or_else(|| state_err("This BitBox holds none of the vault's keys."))?;
    Ok(bitbox_api::btc::make_script_config_multisig(
        threshold,
        &xpubs,
        our_index,
        bitbox_api::pb::btc_script_config::multisig::ScriptType::P2wsh,
    ))
}

pub fn is_policy_registered(
    handle: u64,
    testnet: bool,
    policy: String,
    keys: Vec<PolicyKey>,
    keypath_account: String,
) -> Result<bool, BitBoxError> {
    let kp = parse_keypath(&keypath_account)?;
    with_paired(handle, |p| {
        let config = script_config(p, &policy, &keys, &kp)?;
        block_on(p.btc_is_script_config_registered(coin_for(testnet), &config, Some(&kp)))
            .map_err(device_err)
    })
}

pub fn register_policy(
    handle: u64,
    testnet: bool,
    policy: String,
    keys: Vec<PolicyKey>,
    keypath_account: String,
    name: String,
) -> Result<(), BitBoxError> {
    let kp = parse_keypath(&keypath_account)?;
    with_paired(handle, |p| {
        let config = script_config(p, &policy, &keys, &kp)?;
        block_on(p.btc_register_script_config(
            coin_for(testnet),
            &config,
            Some(&kp),
            bitbox_api::pb::btc_register_script_config_request::XPubType::AutoXpubTpub,
            Some(&name),
        ))
        .map_err(device_err)
    })
}

pub fn policy_address(
    handle: u64,
    testnet: bool,
    policy: String,
    keys: Vec<PolicyKey>,
    keypath: String,
    display: bool,
) -> Result<String, BitBoxError> {
    let kp = parse_keypath(&keypath)?;
    with_paired(handle, |p| {
        let config = script_config(p, &policy, &keys, &kp)?;
        block_on(p.btc_address(coin_for(testnet), &kp, &config, display)).map_err(device_err)
    })
}

pub fn sign_psbt(
    handle: u64,
    testnet: bool,
    psbt_b64: String,
    policy: Option<String>,
    keys: Vec<PolicyKey>,
    keypath_account: Option<String>,
) -> Result<String, BitBoxError> {
    use std::str::FromStr;
    let mut psbt = bitcoin::psbt::Psbt::from_str(psbt_b64.trim())
        .map_err(|e| state_err(&format!("Bad PSBT: {e}")))?;

    // A policy vault CANNOT be inferred from the PSBT: the device refuses to
    // guess a multisig arrangement, so the caller sends the exact policy that
    // was registered. Plain single-sig omits it and the device reads the
    // PSBT's own key origins.
    let account = match (&policy, keypath_account.as_deref()) {
        (None, _) => None,
        (Some(_), None) => return Err(state_err("A policy needs its account keypath.")),
        (Some(_), Some(a)) => Some(parse_keypath(a)?),
    };

    with_paired(handle, |p| {
        let force = match (&policy, &account) {
            (Some(pol), Some(kp)) => Some(bitbox_api::pb::BtcScriptConfigWithKeypath {
                script_config: Some(script_config(p, pol, &keys, kp)?),
                keypath: kp.to_vec(),
            }),
            _ => None,
        };
        block_on(p.btc_sign_psbt(
            coin_for(testnet),
            &mut psbt,
            force.clone(),
            bitbox_api::pb::btc_sign_init_request::FormatUnit::Default,
        ))
        .map_err(device_err)?;
        Ok(psbt.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::parse_wsh_sortedmulti;

    #[test]
    fn plain_sortedmulti_goes_to_the_multisig_config() {
        assert_eq!(
            parse_wsh_sortedmulti("wsh(sortedmulti(2,@0/**,@1/**,@2/**))"),
            Some((2, vec![0, 1, 2]))
        );
        assert_eq!(
            parse_wsh_sortedmulti("wsh(sortedmulti(2,@1/<0;1>/*,@0/**))"),
            Some((2, vec![1, 0]))
        );
    }

    #[test]
    fn everything_else_stays_a_policy() {
        // Miniscript, taproot and non-0/1 change paths are real policies.
        assert_eq!(parse_wsh_sortedmulti("wsh(multi(2,@0/**,@1/**))"), None);
        assert_eq!(parse_wsh_sortedmulti("tr(@0/**,multi_a(2,@1/**,@2/**))"), None);
        assert_eq!(parse_wsh_sortedmulti("wsh(sortedmulti(2,@0/<2;3>/*,@1/**))"), None);
        assert_eq!(
            parse_wsh_sortedmulti("wsh(or_d(sortedmulti(2,@0/**,@1/**),older(100)))"),
            None
        );
    }
}
