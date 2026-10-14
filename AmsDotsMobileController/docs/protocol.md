# AmsDots BLE protocol, version 2

The phone (BLE peripheral) streams motion to the laptop (central). Kotlin
implementation: `shared/src/commonMain/kotlin/com/amsdots/protocol/`. Test data
both implementations must agree on: [`protocol/vectors.txt`](../protocol/vectors.txt).

All numbers are little-endian. "i16" is a signed 16-bit integer.

## GATT layout

| Item | UUID | Properties |
|---|---|---|
| Service | `5be6e39c-9095-468d-a372-5871e621b58b` | |
| Motion characteristic | `5af2dba6-9864-4524-812f-60c35dcb3ff0` | notify |
| Control characteristic | `95978bed-ce1d-404c-932b-25cab3db5b2e` | write |

The phone advertises the service UUID (connectable, four advertisements a second,
high transmit power) and puts its Bluetooth name in the scan response. A 128-bit
UUID nearly fills the 31-byte advertisement, so the name needs the second packet.
The laptop finds phones by the service UUID and lists them by name: the phone's
random address changes every time it starts advertising, so it identifies nothing.
The laptop connects, enables notifications on the motion characteristic and,
optionally, writes control commands. The phone only samples its sensors while the
laptop is subscribed. One laptop at a time; while connected the phone does not
advertise.

## Motion packet (one notification)

| Offset | Size | Field | Unit |
|---|---|---|---|
| 0 | 1 | version: 2 (the phone always sends 2; a receiver also accepts 1, see below) | |
| 1 | 2 | sequence number, wraps at 65536 | |
| 3 | 4 | time of the first sample, since the sensors started, wraps at 2³² | ms |
| 7 | 6 | gravity x, y, z (i16 each) | 1/1000 of 9.81 m/s² |
| 13 | 2 | GPS speed (u16); `0xFFFF` = no speed | cm/s |
| 15 | 2 | age of that speed fix (i16): first sample time minus fix time; 0 when there is no speed | ms |
| 17 | 12 × N | N samples, 1 ≤ N ≤ 4 | |

Sample (12 bytes), in the phone's own axes:

| Offset | Size | Field | Unit |
|---|---|---|---|
| 0 | 6 | linear acceleration x, y, z (i16 each; gravity removed) | mm/s² |
| 6 | 6 | angular velocity x, y, z (i16 each) | mrad/s |

Values saturate at ±32767 (about ±32.7 m/s² or rad/s) instead of wrapping. At
50 Hz consecutive samples are 20 ms apart; the receiver derives each sample's
time from `time of the first sample` plus its index times the interval.

### GPS speed

The speed is the phone's latest GPS fix (`Location.getSpeed()`, the Doppler speed
over ground), repeated in every packet until a newer fix arrives. The fix time is
`time of the first sample − age`, on the same clock as the sample times, so the
receiver can tell a repeated fix (same fix time) from a new one and can place it
among the samples. The age is negative when the fix is newer than the packet's
first sample. The phone stops sending a fix older than 5 s (no GPS, tunnel) and
sends `0xFFFF` instead; a receiver must then fall back to the accelerometer alone.
Speeds above 655.34 m/s saturate at `0xFFFE`, ages at ±32.7 s.

### Version 1

Version 1 has a 13-byte header (no speed and age fields, samples start at offset
13). A receiver accepts both versions and treats a version 1 packet as one with no
speed. The phone only sends version 2.

Packet size is `17 + 12 × N`: 29 bytes for one sample, 65 for four. A notification
carries at most `MTU − 3` bytes, so one sample needs an ATT MTU of at least 32 and
four need 68. The default MTU of 23 is too small: the phone must negotiate a larger
one (it requests 247) and sends as many samples per packet as fit.

A receiver rejects a packet that is shorter than its version's header (`too_short`), has a
version other than 1 or 2 (`bad_version`), has sample bytes that are not a multiple of
12 (`bad_length`) or holds 0 or more than 4 samples (`bad_count`).

## Control writes

| Byte 0 | Meaning | Payload |
|---|---|---|
| `0x01` | set sample rate | 1 byte: Hz, 10 to 100 |

Unknown or malformed writes are ignored.

## Changing the protocol

Change the version byte for any incompatible layout change, update the vectors
file, and keep the Kotlin and Rust implementations passing it.
