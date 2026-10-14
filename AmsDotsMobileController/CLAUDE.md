# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Android half of a "vehicle motion cues" tool: streams phone motion sensors over BLE to the Linux overlay in `../AmsDotsLinux`. `ROADMAP.md` has the design decisions and milestone status (M0-M1 done; M2-M4 coded but not yet verified on a phone); `README.md` has the layout and commands.

## Commands
- `./gradlew :androidApp:assembleDebug` — build the APK (works with the installed JDK 27 despite the daemon-JVM file asking for 21)
- `./gradlew :shared:testAndroidHostTest` — all unit tests; add `--tests '*PacketCodecTest'` for one class. Test results: `shared/build/test-results/testAndroidHostTest/*.xml`
- `adb` is not on PATH by default: `export PATH=$PATH:$HOME/Android/Sdk/platform-tools`
- Test phone: Xiaomi 22101316G, Android 14 (HyperOS); expect aggressive background killing, so background work needs the app's Autostart/No-restrictions battery settings.

## Architecture
- `shared` is a Kotlin Multiplatform module using the Android-KMP-library plugin (`androidMain`, `androidHostTest` source sets, no `androidTest`). Platform-free logic goes in `commonMain`; sensors, BLE and the service go in `androidMain`.
- The wire format has three authorities that must agree: `docs/protocol.md` (spec), `shared/.../protocol/` (Kotlin) and `protocol/vectors.txt` (golden vectors, produced by an independent Python encoder; the Rust receiver in `../AmsDotsLinux` will test against the same file). Change the spec and vectors first.
- Phone sends raw sensor-frame data plus gravity; conversion to vehicle axes happens on the laptop.
- Runtime shape: `MotionService` (foreground, `connectedDevice`) owns a `HandlerThread`; sensor events and GATT callbacks are all posted onto it, so `stream/` classes are single-threaded and lock-free. Pipeline: `MotionSensors` -> `SensorFuser` (one sample per gyro event, accel interpolated) -> `MotionStreamer` (packets, one notification in flight, drops oldest when behind) -> `GattPeripheral`. The laptop is found by service UUID; the phone advertises no name.
- Debug figures on the screen: `ReadoutMeter` (acceleration split by gravity, turning rate, tilt; every 250 ms while the sensors run) and `SpeedMeter` (GPS speed and its change). Location is optional (asked at Start, not required); with it the service adds the `location` foreground type so GPS works with the screen off. Nothing of it goes to the laptop.
