# Roadmap: AmsDots phone sender

The phone reads its motion sensors and streams them over Bluetooth Low Energy
(BLE) to the Linux overlay in `../AmsDotsLinux`, which turns them into moving
dots. The app must use very little energy and keep working with the screen off
or another app in the foreground.

## Design decisions

**Transport: BLE GATT notifications.** The phone is the *peripheral*: it runs a
GATT server and advertises a custom service named "AmsDots" with the lowest-power
advertising mode. The laptop is the *central*: it connects and subscribes to the
*motion* characteristic (notify). A *control* characteristic (write) lets the
laptop choose the sample rate. No pairing is needed, and no tethering has to be
switched on by hand. Wire format: [`docs/protocol.md`](docs/protocol.md).

**Sensors run only while someone listens.** Sensors are registered when the
laptop subscribes and unregistered when it disconnects. Idle cost is only the
slow advertising.

**Background operation.** A foreground service of type `connectedDevice` with an
ongoing notification (required since Android 14). Permissions:
`BLUETOOTH_ADVERTISE`, `BLUETOOTH_CONNECT`, `POST_NOTIFICATIONS`,
`FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_CONNECTED_DEVICE`. `WAKE_LOCK` only if
measurements show it is needed. The app asks the user to exempt it from battery
optimisation.

**Energy strategy.**
- Linear acceleration + gyroscope + gravity at about 50 Hz (below the 200 Hz
  limit that needs an extra permission).
- *Wake-up* sensor variants with hardware batching
  (`registerListener(listener, sensor, samplingPeriodUs, maxReportLatencyUs)` with
  a latency of about 80-100 ms), so the CPU wakes about 10 times a second instead
  of staying awake.
- Several samples per BLE notification; larger ATT MTU (target 247) and balanced
  connection priority.
- No UI work while the screen is off.
- Open question to settle in M2: plain (non-wake-up) sensors stop delivering when
  the CPU suspends, so screen-off delivery is tested on the real phone before
  anything is built on top of it.

**Raw data from the phone, fusion on the laptop.** The phone sends sensor-frame
values plus the gravity vector. Converting to vehicle axes (gravity gives "up",
yaw rate = gyro projected on "up", forward axis from a calibrate command) happens
in the Rust app, so the phone does almost no work.

**Kotlin Multiplatform split.** `shared/commonMain`: packet codec, motion sample
type, link state model, Compose UI. `androidMain`: sensor source, GATT server and
advertiser, foreground service, permission handling. No iOS target planned.

## Milestones

| | Milestone | Done when |
|---|---|---|
| M0 | **Toolchain and hello-phone** (done) | `./gradlew :androidApp:assembleDebug` succeeds (check JDK 27 against AGP 9.1.1 and `gradle/gradle-daemon-jvm.properties`, SDK path in `local.properties`); `adb` on PATH (`~/Android/Sdk/platform-tools`); USB debugging on; the template app runs on the phone; demo UI removed. |
| M1 | **Protocol and codec** (done) | `docs/protocol.md` final; Kotlin encoder/decoder in `shared/commonMain` with unit tests; shared golden vectors in `protocol/vectors.txt` (the Rust decoder will test against the same file). |
| M2 | **Sensor foreground service (no Bluetooth)** | Service with notification and permission flow logs achieved sample rate and jitter. Verified with screen off and with another app in the foreground. First battery measurement (30 min screen-off, `adb shell dumpsys batterystats`) decides wake-up sensors + batching versus a wake lock. |
| M3 | **BLE GATT server** | Advertises, accepts a central, notifies a counter first, then real packets. Verified from the laptop with `bluetoothctl` (`menu gatt`, subscribe) and `btmon`. Sensors start/stop on subscribe/unsubscribe. |
| M4 | **Energy tuning** | Compare at least three profiles (batching latency, connection priority, rate) by 1 h screen-off battery drain; pick defaults; expose the rate through the control characteristic. |
| M5 | **App UX** | One status screen (armed / advertising / connected / rate), start-stop, permission and battery-optimisation flows, notification stop action, Quick Settings tile, recovery after process restart. |
| M6 | **Laptop receiver** (in `AmsDotsLinux`) | `BleSource` using the `bluer` crate: scan, connect, subscribe, auto-reconnect; disconnected means zero motion so the dots settle. Decoder tested against `protocol/vectors.txt`. `Fusion` module with calibrate command. Replaces the CSV replay as the default source (CSV stays for development). |
| M7 | **Field test and tuning** | A real drive; sessions recorded to the CSV format of `AmsDotsLinux/tests/data/drive.csv` for replay and for tuning gains. |

## Main risks

- Manufacturer background-kill policies (battery managers) stopping the service.
- Android 14/15 restrictions on starting foreground services from the background.
- BLE GATT error 133 and reconnect races on both sides.
- Notification throttling when the connection interval is long.
- Screen-off sensor delivery (decided in M2).
- Phone orientation and calibration inside the vehicle.
- BlueZ access for the laptop daemon (D-Bus, user session).
