# AmsDots mobile sender

Android app that streams the phone's motion sensors over Bluetooth Low Energy to
the Linux overlay in `../AmsDotsLinux`, which turns them into the moving dots
(a copy of the iPhone's *Vehicle Motion Cues*). The plan, design decisions and
milestone status are in [ROADMAP.md](ROADMAP.md); the wire format is in
[docs/protocol.md](docs/protocol.md).

## Layout

| Path | Contents |
|---|---|
| `shared/src/commonMain` | Platform-independent code: `protocol/` (packet codec, control commands), `stream/` (sensor fusion, packet pacing, rate meter, link status) and the Compose UI (`App.kt`). |
| `shared/src/androidMain` | Android-specific code in `android/`: `MotionSensors`, `GattPeripheral` (GATT server + advertiser), `MotionService` (foreground service), `ServiceState`. |
| `shared/src/commonTest` | Unit tests for the codec and the `stream/` logic. |
| `shared/src/androidHostTest` | Tests that need the JVM, e.g. `GoldenVectorsTest`, which reads `protocol/vectors.txt`. |
| `androidApp` | The Android application module (manifest, activity). |
| `protocol/vectors.txt` | Test vectors for the packet format, shared with the Rust receiver. |

## Commands

```sh
export PATH=$PATH:$HOME/Android/Sdk/platform-tools    # for adb

./gradlew :androidApp:assembleDebug                   # build the APK
adb install -r androidApp/build/outputs/apk/debug/androidApp-debug.apk
adb shell am start -n com.amsdots/.MainActivity       # launch it

./gradlew :shared:testAndroidHostTest                 # all unit tests (common + host)
./gradlew :shared:testAndroidHostTest --tests '*PacketCodecTest'   # one class
```

The protocol vectors are the contract between phone and laptop: after changing
the packet format, update `docs/protocol.md` and `protocol/vectors.txt` first.
