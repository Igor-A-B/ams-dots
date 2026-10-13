//! Bluetooth receiver: finds the phone, connects, subscribes to its motion
//! notifications and feeds them to [`Live`].
//!
//! It runs on its own thread with a small async runtime, because the BlueZ
//! library (`bluer`) is async and the overlay's Wayland loop is not. The two
//! meet only in [`Live`]. The thread never gives up: if the phone goes away
//! (out of range, app stopped, Bluetooth off) the dots settle and the thread
//! goes back to scanning, so the overlay can simply stay running in the
//! background. `ams-dots-linux disconnect` pauses it, `connect` resumes, and
//! `select` points it at another phone (the session ends and a new one starts
//! for the new target, see [`Live::target`]).
//!
//! Finding the phone: the phone advertises the AmsDots service UUID with a
//! random address that changes whenever its link is (re)started, and BlueZ
//! keeps old sightings in a cache. A device only counts if it has a signal
//! strength: BlueZ clears that when a scan stops, so cached sightings from an
//! earlier scan do not. The chosen phone is recognised by its Bluetooth name.
//! Everything heard is also reported to [`Live`] for the `phones` command.
//!
//! Feedback: every change of the link is printed on stderr and shown as a
//! desktop notification (`notify-send`, if installed), so you see that the two
//! apps found each other without looking at a terminal.

use crate::control;
use crate::motion::{LinkState, Live};
use crate::picker::{self, Candidate, Target};
use crate::protocol::{self, CONTROL_UUID, MOTION_UUID, SERVICE_UUID};
use bluer::{Adapter, Device, DeviceEvent, DeviceProperty, DiscoveryFilter, DiscoveryTransport, Uuid, gatt::remote::Characteristic};
use futures::{StreamExt, pin_mut};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{interval, sleep, timeout};

/// Pause before looking for the phone again after a failure or a lost link.
const RETRY_DELAY: Duration = Duration::from_millis(500);
/// How often the list of sightings is looked at while searching.
const POLL: Duration = Duration::from_millis(250);
/// Safety net for noticing a dead link when BlueZ sends no event for it.
const LINK_CHECK: Duration = Duration::from_secs(1);
/// Connecting normally takes about a second; give up after this.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
/// Subscribed but silent this long: probably the phone could not negotiate a big enough MTU.
const SILENCE_HINT: Duration = Duration::from_secs(8);
/// Looking for a named phone this long without finding it: say so, once.
const WAITING_HINT: Duration = Duration::from_secs(10);

fn service_uuid() -> Uuid {
    Uuid::parse_str(SERVICE_UUID).expect("valid service UUID")
}

/// Starts the receiver thread, which follows the phone in [`Live::target`]. `rate_hz`, if given, is sent to
/// the phone after each connect. With `remember`, the phone that connects is stored as the last known one,
/// so the next start connects to it by itself.
pub fn spawn(live: Arc<Live>, rate_hz: Option<u8>, remember: bool) {
    std::thread::Builder::new()
        .name("ams-dots-ble".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("async runtime");
            runtime.block_on(async {
                tokio::join!(connection_loop(live.clone(), rate_hz, remember), control::serve(live));
            });
        })
        .expect("spawn the Bluetooth thread");
}

/// Scans for `window` and returns the phones running the AmsDots app, strongest signal first.
/// Blocking; for the start-up picker.
pub fn scan(window: Duration) -> Result<Vec<Candidate>, String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let (_session, adapter) = open_adapter().await.map_err(|e| e.to_string())?;
        let _discovery = adapter.discover_devices().await.map_err(|e| e.to_string())?;
        let deadline = Instant::now() + window;
        let mut seen: Vec<Candidate> = Vec::new();
        while Instant::now() < deadline {
            sleep(POLL).await;
            for (_, candidate) in fresh_phones(&adapter).await.map_err(|e| e.to_string())? {
                match seen.iter_mut().find(|c| c.address == candidate.address) {
                    Some(known) => *known = candidate,
                    None => seen.push(candidate),
                }
            }
        }
        // The same phone can show up under two addresses if its link restarted during the scan: keep the stronger.
        seen.sort_by_key(|c| std::cmp::Reverse(c.rssi.unwrap_or(i16::MIN)));
        let mut names = HashSet::new();
        seen.retain(|c| c.name.is_none() || names.insert(c.name.clone()));
        Ok(seen)
    })
}

/// The adapter, set up for finding phones. No UUID is given to the discovery filter on purpose: with one,
/// bluetoothd 5.87 crashes (SIGSEGV) as soon as the phone's advertisement matches it, so phones are
/// recognised here by their service UUID instead. The session must stay alive as long as the adapter is used:
/// dropping it closes the D-Bus connection.
async fn open_adapter() -> bluer::Result<(bluer::Session, Adapter)> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;
    adapter
        .set_discovery_filter(DiscoveryFilter {
            transport: DiscoveryTransport::Le,
            ..Default::default()
        })
        .await?;
    Ok((session, adapter))
}

/// The devices that offer the AmsDots service and have been heard from (they have a signal strength).
async fn fresh_phones(adapter: &Adapter) -> bluer::Result<Vec<(Device, Candidate)>> {
    let mut phones = Vec::new();
    for address in adapter.device_addresses().await? {
        let device = adapter.device(address)?;
        if !device.uuids().await?.unwrap_or_default().contains(&service_uuid()) {
            continue;
        }
        // Heard just now (it has a signal strength), or still connected from an earlier run that was killed:
        // a connected phone no longer advertises, so it would never be heard again.
        let rssi = device.rssi().await?;
        if rssi.is_none() && !device.is_connected().await.unwrap_or(false) {
            continue;
        }
        let candidate = Candidate { name: device.name().await?, address: address.to_string(), rssi };
        phones.push((device, candidate));
    }
    Ok(phones)
}

/// Makes Ctrl+C and SIGTERM disconnect the phone before the process ends. Killing the process while connected
/// would leave the connection open in BlueZ: the phone then stops advertising and the next run cannot find it.
pub fn disconnect_on_exit(live: Arc<Live>) {
    use signal_hook::consts::{SIGINT, SIGTERM};
    let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGINT, SIGTERM]) else { return };
    std::thread::spawn(move || {
        if signals.forever().next().is_some() {
            live.set_enabled(false);
            let deadline = Instant::now() + Duration::from_secs(2);
            while live.is_connected() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            std::process::exit(130);
        }
    });
}

/// Prints a message and, if possible, shows it as a desktop notification.
fn announce(message: &str) {
    eprintln!("ams-dots: {message}");
    // Fire and forget: no notification daemon or no notify-send just means no popup.
    let _ = tokio::process::Command::new("notify-send")
        .args(["--app-name=AmsDots", "--expire-time=3000", "AmsDots", message])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Resolves when the user turns the receiver off (`disconnect`) or picks another phone (`select`, which
/// changes the target generation). Wake-ups that change neither are ignored.
async fn until_changed(live: &Live, generation: u64) {
    loop {
        if !live.is_enabled() || live.target().1 != generation {
            return;
        }
        live.toggled().await;
    }
}

/// Why a session ended without an error.
enum Ended {
    /// The phone went away.
    Lost,
    /// The user ran `disconnect` or `select`.
    Switched,
}

/// Connects, streams until the link drops, and starts over, forever.
async fn connection_loop(live: Arc<Live>, rate_hz: Option<u8>, remember: bool) {
    let mut last_error = String::new();
    loop {
        if !live.is_enabled() {
            live.set_link(LinkState::Paused);
            while !live.is_enabled() {
                live.toggled().await;
            }
        }
        live.set_link(LinkState::Scanning);
        let (target, generation) = live.target();
        match stream_once(&live, rate_hz, &target, generation, remember).await {
            Ok(Ended::Lost) => announce("phone disconnected, looking for it again"),
            Ok(Ended::Switched) if live.is_enabled() => announce(&format!("switching to {}", live.target().0.describe())),
            Ok(Ended::Switched) => announce("disconnected"),
            Err(err) => {
                // The same failure every second (e.g. Bluetooth off) is reported once.
                let message = err.to_string();
                if message != last_error {
                    announce(&format!("bluetooth problem: {message}"));
                    last_error = message;
                }
            }
        }
        live.set_link(if live.is_enabled() { LinkState::Scanning } else { LinkState::Paused });
        sleep(RETRY_DELAY).await;
    }
}

/// One whole session: scan, connect, subscribe, stream. Returns when the link ends.
async fn stream_once(
    live: &Live,
    rate_hz: Option<u8>,
    target: &Target,
    generation: u64,
    remember: bool,
) -> Result<Ended, Box<dyn std::error::Error>> {
    let (_session, adapter) = open_adapter().await?;
    // Scanning stops when this is dropped, which happens once the phone is connected.
    let discovery = adapter.discover_devices().await?;
    let (device, candidate) = tokio::select! {
        found = find_phone(&adapter, target, live) => found?,
        () = until_changed(live, generation) => return Ok(Ended::Switched),
    };
    live.set_link(LinkState::Connecting);
    live.set_phone(&candidate);
    eprintln!("ams-dots: found {} ({}), connecting", candidate.label(), candidate.address);

    let setup = async {
        if !device.is_connected().await? {
            timeout(CONNECT_TIMEOUT, device.connect()).await.map_err(|_| "connecting timed out")??;
        }
        let (motion, control_char) = find_characteristics(&device).await?;
        if let (Some(hz), Some(control_char)) = (rate_hz, &control_char) {
            control_char.write(&protocol::set_rate_command(hz)).await?;
            eprintln!("ams-dots: asked the phone for {hz} Hz");
        }
        let notifications = motion.notify().await?;
        Ok::<_, Box<dyn std::error::Error>>(notifications)
    };
    let setup = tokio::select! {
        result = setup => Some(result),
        () = until_changed(live, generation) => None,
    };
    let notifications = match setup {
        Some(Ok(notifications)) => notifications,
        Some(Err(err)) => {
            abandon(&device).await;
            return Err(err);
        }
        None => {
            abandon(&device).await;
            return Ok(Ended::Switched);
        }
    };
    drop(discovery);
    pin_mut!(notifications);
    let device_events = device.events().await?;
    pin_mut!(device_events);

    live.set_link(LinkState::Connected);
    announce(&format!("{} connected, receiving motion", candidate.label()));
    if let (true, Some(name)) = (remember, &candidate.name) {
        picker::remember(name);
    }

    let mut check = interval(LINK_CHECK);
    let (mut packets, mut errors, mut hinted) = (0u64, 0u64, false);
    let started = Instant::now();
    let ended = loop {
        tokio::select! {
            bytes = notifications.next() => {
                let Some(bytes) = bytes else { break Ended::Lost };
                match protocol::decode(&bytes) {
                    Ok(packet) => {
                        packets += 1;
                        let outcome = live.push(&packet, Instant::now());
                        if outcome.lost > 0 {
                            eprintln!("ams-dots: {} packet(s) lost", outcome.lost);
                        }
                        if let Some(state) = outcome.forward_learned {
                            announce(&format!("forward direction: {}", state.text()));
                        }
                    }
                    Err(err) => {
                        errors += 1;
                        if errors <= 3 {
                            eprintln!("ams-dots: ignoring a bad packet ({err}), {} bytes", bytes.len());
                        }
                    }
                }
            }
            event = device_events.next() => match event {
                Some(DeviceEvent::PropertyChanged(DeviceProperty::Connected(false))) | None => break Ended::Lost,
                Some(_) => {}
            },
            _ = check.tick() => {
                if !device.is_connected().await.unwrap_or(false) {
                    break Ended::Lost;
                }
                if packets == 0 && !hinted && started.elapsed() > SILENCE_HINT {
                    hinted = true;
                    eprintln!("ams-dots: connected but no motion yet; is the phone app started, and did the link get an MTU of at least 64?");
                }
            }
            () = until_changed(live, generation) => break Ended::Switched,
        }
    };
    abandon(&device).await;
    live.set_link(LinkState::Scanning);
    Ok(ended)
}

/// Ends a connection or a connection attempt cleanly. Without this a timed-out attempt stays open in BlueZ
/// and the next one fails with "operation in progress".
/// (Do not "tidy up" with `Adapter::remove_device`: it crashes bluetoothd 5.87.)
async fn abandon(device: &Device) {
    let _ = device.disconnect().await;
}

/// Waits until a phone that matches `target` has been seen. Every phone heard is reported to `live` (for
/// `phones`); a named phone that stays away is mentioned once, with the other phones in range.
async fn find_phone(adapter: &Adapter, target: &Target, live: &Live) -> bluer::Result<(Device, Candidate)> {
    let started = Instant::now();
    let mut hinted = false;
    loop {
        let phones = fresh_phones(adapter).await?;
        let heard: Vec<Candidate> = phones.iter().map(|(_, candidate)| candidate.clone()).collect();
        live.report_sightings(&heard, Instant::now());
        if let Some((device, candidate)) = phones.into_iter().find(|(_, candidate)| target.matches(candidate)) {
            return Ok((device, candidate));
        }
        if !hinted && started.elapsed() > WAITING_HINT {
            hinted = true;
            let others: Vec<String> = heard.iter().map(Candidate::label).collect();
            announce(&if others.is_empty() {
                format!("still looking for {}: start the app on the phone", target.describe())
            } else {
                format!("still looking for {}; other phones in range: {}. `ams-dots-linux select` switches", target.describe(), others.join(", "))
            });
        }
        sleep(POLL).await;
    }
}

/// The motion characteristic (required) and the control characteristic (optional) of the phone's service.
async fn find_characteristics(device: &Device) -> Result<(Characteristic, Option<Characteristic>), Box<dyn std::error::Error>> {
    let (service, motion_uuid, control_uuid) = (service_uuid(), Uuid::parse_str(MOTION_UUID)?, Uuid::parse_str(CONTROL_UUID)?);
    // BlueZ resolves the services a moment after connecting.
    for _ in 0..30 {
        for gatt_service in device.services().await? {
            if gatt_service.uuid().await? != service {
                continue;
            }
            let (mut motion, mut control_char) = (None, None);
            for characteristic in gatt_service.characteristics().await? {
                let uuid = characteristic.uuid().await?;
                if uuid == motion_uuid {
                    motion = Some(characteristic);
                } else if uuid == control_uuid {
                    control_char = Some(characteristic);
                }
            }
            if let Some(motion) = motion {
                return Ok((motion, control_char));
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
    Err("the phone does not offer the AmsDots motion characteristic".into())
}
