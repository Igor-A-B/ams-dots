//! Bluetooth receiver: finds the phone, connects, subscribes to its motion
//! notifications and feeds them to [`Live`].
//!
//! It runs on its own thread with a small async runtime, because the BlueZ
//! library (`bluer`) is async and the overlay's Wayland loop is not. The two
//! meet only in [`Live`]. The thread never gives up: if the phone goes away
//! (out of range, app stopped, Bluetooth off) the dots settle and the thread
//! goes back to scanning, so the overlay can simply stay running in the
//! background.

use crate::control;
use crate::motion::Live;
use crate::protocol::{self, CONTROL_UUID, MOTION_UUID, SERVICE_UUID};
use bluer::{Adapter, AdapterEvent, Device, DiscoveryFilter, DiscoveryTransport, Uuid, gatt::remote::Characteristic};
use futures::{StreamExt, pin_mut};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{interval, sleep, timeout};

/// Pause before looking for the phone again after a failure or a lost link.
const RETRY_DELAY: Duration = Duration::from_secs(3);
/// How often to check that the link is still up.
const LINK_CHECK: Duration = Duration::from_secs(2);
/// Connecting normally takes about a second; give up after this.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Subscribed but silent this long: probably the phone could not negotiate a big enough MTU.
const SILENCE_HINT: Duration = Duration::from_secs(8);

/// Starts the receiver thread. `rate_hz`, if given, is sent to the phone after each connect.
pub fn spawn(live: Arc<Live>, rate_hz: Option<u8>) {
    std::thread::Builder::new()
        .name("ams-dots-ble".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("async runtime");
            runtime.block_on(async {
                tokio::join!(connection_loop(live.clone(), rate_hz), control::serve(live));
            });
        })
        .expect("spawn the Bluetooth thread");
}

/// Connects, streams until the link drops, and starts over, forever.
async fn connection_loop(live: Arc<Live>, rate_hz: Option<u8>) {
    let mut last_error = String::new();
    loop {
        match stream_once(&live, rate_hz).await {
            Ok(()) => eprintln!("ams-dots: phone disconnected, looking for it again"),
            Err(err) => {
                // Same failure every few seconds (e.g. Bluetooth off) is reported once.
                let message = err.to_string();
                if message != last_error {
                    eprintln!("ams-dots: bluetooth: {message}");
                    last_error = message;
                }
            }
        }
        live.set_connected(false);
        sleep(RETRY_DELAY).await;
    }
}

/// One whole session: scan, connect, subscribe, stream. Returns when the link ends.
async fn stream_once(live: &Live, rate_hz: Option<u8>) -> Result<(), Box<dyn std::error::Error>> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    let service = Uuid::parse_str(SERVICE_UUID)?;
    let device = find_phone(&adapter, service).await?;
    eprintln!("ams-dots: found the phone ({}), connecting", device.address());

    if !device.is_connected().await? {
        timeout(CONNECT_TIMEOUT, device.connect()).await.map_err(|_| "connecting timed out")??;
    }
    let (motion, control_char) = find_characteristics(&device, service).await?;

    if let (Some(hz), Some(control_char)) = (rate_hz, &control_char) {
        control_char.write(&protocol::set_rate_command(hz)).await?;
        eprintln!("ams-dots: asked the phone for {hz} Hz");
    }

    let notifications = motion.notify().await?;
    pin_mut!(notifications);
    live.set_connected(true);
    eprintln!("ams-dots: connected, receiving motion");

    let mut check = interval(LINK_CHECK);
    let (mut packets, mut errors, mut hinted) = (0u64, 0u64, false);
    let started = Instant::now();
    loop {
        tokio::select! {
            bytes = notifications.next() => {
                let Some(bytes) = bytes else { break };
                match protocol::decode(&bytes) {
                    Ok(packet) => {
                        packets += 1;
                        let outcome = live.push(&packet, Instant::now());
                        if outcome.lost > 0 {
                            eprintln!("ams-dots: {} packet(s) lost", outcome.lost);
                        }
                        if let Some(event) = outcome.calibration {
                            eprintln!("ams-dots: calibration: {event:?}");
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
            _ = check.tick() => {
                if !device.is_connected().await.unwrap_or(false) {
                    break;
                }
                if packets == 0 && !hinted && started.elapsed() > SILENCE_HINT {
                    hinted = true;
                    eprintln!("ams-dots: connected but no motion yet; is the phone app started, and did the link get an MTU of at least 64?");
                }
            }
        }
    }
    let _ = device.disconnect().await;
    Ok(())
}

/// Waits until a device that offers the AmsDots service shows up.
async fn find_phone(adapter: &Adapter, service: Uuid) -> bluer::Result<Device> {
    let filter = DiscoveryFilter {
        uuids: HashSet::from([service]),
        transport: DiscoveryTransport::Le,
        ..Default::default()
    };
    adapter.set_discovery_filter(filter).await?;
    let events = adapter.discover_devices().await?;
    pin_mut!(events);
    while let Some(event) = events.next().await {
        if let AdapterEvent::DeviceAdded(address) = event {
            let device = adapter.device(address)?;
            if device.uuids().await?.unwrap_or_default().contains(&service) {
                return Ok(device);
            }
        }
    }
    Err(bluer::Error { kind: bluer::ErrorKind::Failed, message: "the Bluetooth scan ended".into() })
}

/// The motion characteristic (required) and the control characteristic (optional) of the phone's service.
async fn find_characteristics(device: &Device, service: Uuid) -> Result<(Characteristic, Option<Characteristic>), Box<dyn std::error::Error>> {
    let (motion_uuid, control_uuid) = (Uuid::parse_str(MOTION_UUID)?, Uuid::parse_str(CONTROL_UUID)?);
    // BlueZ resolves the services a moment after connecting.
    for _ in 0..20 {
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
        sleep(Duration::from_millis(500)).await;
    }
    Err("the phone does not offer the AmsDots motion characteristic".into())
}
