# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Android half of a "vehicle motion cues" tool: streams phone motion sensors over BLE to the Linux overlay in `../AmsDotsLinux`. `ROADMAP.md` has the design decisions and milestone status (M0 and M1 done); `README.md` has the layout and commands.

## Commands
- `./gradlew :androidApp:assembleDebug` — build the APK (works with the installed JDK 27 despite the daemon-JVM file asking for 21)
- `./gradlew :shared:testAndroidHostTest` — all unit tests; add `--tests '*PacketCodecTest'` for one class. Test results: `shared/build/test-results/testAndroidHostTest/*.xml`
- `adb` is not on PATH by default: `export PATH=$PATH:$HOME/Android/Sdk/platform-tools`
- Test phone: Xiaomi 22101316G, Android 14 (HyperOS); expect aggressive background killing, so background work needs the app's Autostart/No-restrictions battery settings.

## Architecture
- `shared` is a Kotlin Multiplatform module using the Android-KMP-library plugin (`androidMain`, `androidHostTest` source sets, no `androidTest`). Platform-free logic goes in `commonMain`; sensors, BLE and the service go in `androidMain`.
- The wire format has three authorities that must agree: `docs/protocol.md` (spec), `shared/.../protocol/` (Kotlin) and `protocol/vectors.txt` (golden vectors, produced by an independent Python encoder; the Rust receiver in `../AmsDotsLinux` will test against the same file). Change the spec and vectors first.
- Phone sends raw sensor-frame data plus gravity; conversion to vehicle axes happens on the laptop.
