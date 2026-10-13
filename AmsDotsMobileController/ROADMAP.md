# Roadmap: AmsDots phone sender

The phone reads its motion sensors and streams them over Bluetooth Low Energy
(BLE) to the Linux overlay in `../AmsDotsLinux`, which turns them into moving
dots. The app must use very little energy and keep working with the screen off
or another app in the foreground.

## Design decisions

**Transport: BLE GATT notifications.** The phone is the *peripheral*: it runs a
GATT server and advertises a custom service (four advertisements a second, its name in the scan response). The laptop is the *central*: it connects and subscribes to the
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
- Several samples per BLE notification. The central (BlueZ on the laptop) starts
  the MTU exchange and picks the connection parameters; the phone as a
  peripheral cannot request a connection priority, so M4 tunes only batching
  latency and rate on the phone side.
- No UI work while the screen is off.
- Open question to settle in M2: plain (non-wake-up) sensors stop delivering when
  the CPU suspends, so screen-off delivery is tested on the real phone before
  anything is built on top of it.

**Raw data from the phone, fusion on the laptop.** The phone sends sensor-frame
values plus the gravity vector. Converting to vehicle axes (gravity gives "up",
yaw rate = gyro projected on "up", forward axis learned automatically from the driving itself) happens
in the Rust app, so the phone does almost no work.

**Kotlin Multiplatform split.** `shared/commonMain`: packet codec, motion sample
type, link state model, Compose UI. `androidMain`: sensor source, GATT server and
advertiser, foreground service, permission handling. No iOS target planned.

## Milestones

| | Milestone | Done when |
|---|---|---|
| M0 | **Toolchain and hello-phone** (done) | `./gradlew :androidApp:assembleDebug` succeeds (check JDK 27 against AGP 9.1.1 and `gradle/gradle-daemon-jvm.properties`, SDK path in `local.properties`); `adb` on PATH (`~/Android/Sdk/platform-tools`); USB debugging on; the template app runs on the phone; demo UI removed. |
| M1 | **Protocol and codec** (done) | `docs/protocol.md` final; Kotlin encoder/decoder in `shared/commonMain` with unit tests; shared golden vectors in `protocol/vectors.txt` (the Rust decoder will test against the same file). |
| M2 | **Sensor foreground service (no Bluetooth)** (code done, device checks open) | Service with notification and permission flow logs achieved sample rate and jitter. Verified with screen off and with another app in the foreground. First battery measurement (30 min screen-off, `adb shell dumpsys batterystats`) decides wake-up sensors + batching versus a wake lock. |
| M3 | **BLE GATT server** (code done, device checks open) | Advertises, accepts a central, notifies a counter first, then real packets. Verified from the laptop with `bluetoothctl` (`menu gatt`, subscribe) and `btmon`. Sensors start/stop on subscribe/unsubscribe. |
| M4 | **Energy tuning** (rate control done, measurements open) | Compare at least three profiles (batching latency, connection priority, rate) by 1 h screen-off battery drain; pick defaults; expose the rate through the control characteristic. |
| M5 | **App UX** (not started; the MVP has only a Start/Stop button and a status line) | One status screen (armed / advertising / connected / rate), start-stop, permission and battery-optimisation flows, notification stop action, Quick Settings tile, recovery after process restart. |
| M6 | **Laptop receiver** (in `AmsDotsLinux`) (done, tried with the phone) | `BleSource` using the `bluer` crate: scan, connect, subscribe, auto-reconnect; disconnected means zero motion so the dots settle. Decoder tested against `protocol/vectors.txt`. `Fusion` module that learns the forward direction by itself (no calibrate command), `status`/`connect`/`disconnect` commands and desktop notifications. Phone choice: a list the first time, then the laptop reconnects to the last phone by itself; `phones`/`select`/`forget` switch phones while the overlay runs. Replaces the CSV replay as the default source (CSV stays for development). |
| M7 | **Field test and tuning** | A real drive; sessions recorded to the CSV format of `AmsDotsLinux/tests/data/drive.csv` for replay and for tuning gains. |

## Main risks

- Manufacturer background-kill policies (battery managers) stopping the service.
- Android 14/15 restrictions on starting foreground services from the background.
- BLE GATT error 133 and reconnect races on both sides.
- Notification throttling when the connection interval is long.
- Screen-off sensor delivery (decided in M2).
- Phone orientation inside the vehicle (learned automatically; unproven in a real car).
- BlueZ access for the laptop daemon (D-Bus, user session).

## Status: MVP code complete, device verification open

M0-M4 are implemented and unit-tested (fuser, streamer, rate meter, codec) and the APK
builds, but **nothing has run on the phone yet** (none was attached during the session
that wrote it). What is left before the MVP counts as working:

1. **M2 checks.** Install, tick "Sensor test (no Bluetooth)", Start. `adb logcat -s AmsDots`
   prints the sensors found (wake-up or not, FIFO size) and every 5 s the achieved rate,
   jitter and largest gap. Repeat with the screen off and another app in front. If the log
   says "not all sensors are wake-up sensors", the service holds a partial wake lock while
   streaming; compare `adb shell dumpsys batterystats` over 30 min with and without it.
2. **M3 checks.** Start without the test switch. On the laptop: `bluetoothctl`, `scan on`,
   find the device advertising `5be6e39c-...`, `connect`, `menu gatt`, `select-attribute`
   the motion characteristic, `notify on`. The app should switch to Streaming and
   `btmon` should show 25-61 byte notifications. Write `01 14` (20 Hz) to the control
   characteristic and watch the target rate change.
3. **M4.** Pick the defaults (`MAX_REPORT_LATENCY_US` in `MotionSensors`, `maxLatencyMs` in
   `MotionStreamer`, `LinkStatus.DEFAULT_RATE_HZ`) from 1 h screen-off drain at three settings.
4. Xiaomi/HyperOS: set the app's battery saver to "No restrictions" and enable Autostart,
   or the system kills the service.
