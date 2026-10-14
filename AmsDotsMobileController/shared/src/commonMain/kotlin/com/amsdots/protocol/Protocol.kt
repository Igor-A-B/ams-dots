package com.amsdots.protocol

/**
 * Constants of the AmsDots BLE protocol. The wire format is specified in
 * `docs/protocol.md`; this file and the Rust receiver must both follow it.
 */
object Protocol {
    /** Version byte at the start of every motion packet the phone sends. */
    const val VERSION = 2

    /** The first version, without the GPS speed; still decoded, never sent. */
    const val VERSION_1 = 1

    /** Bytes before the first sample: version, sequence, time, gravity, speed, speed age. */
    const val HEADER_SIZE = 17

    /** Header of a version 1 packet: no speed and age. */
    const val HEADER_SIZE_V1 = 13

    /** Wire value of the speed field when there is no GPS speed. */
    const val NO_SPEED = 0xFFFF

    /** A GPS fix older than this (ms) is not sent: the receiver falls back to the accelerometer. */
    const val MAX_FIX_AGE_MS = 5_000

    /** Largest speed on the wire (cm/s); faster saturates. */
    const val MAX_SPEED_CM_S = 0xFFFE

    /** Bytes per sample: six 16-bit values. */
    const val SAMPLE_SIZE = 12

    /** Most samples one packet may carry. */
    const val MAX_SAMPLES = 4

    /** Bytes the ATT layer adds to every notification on top of its payload. */
    const val ATT_OVERHEAD = 3

    /** Lowest and highest sample rate (Hz) the control characteristic accepts. */
    const val MIN_RATE_HZ = 10
    const val MAX_RATE_HZ = 100

    /** GATT service the phone advertises and the laptop scans for (the phone advertises no name). */
    const val SERVICE_UUID = "5be6e39c-9095-468d-a372-5871e621b58b"

    /** Notify characteristic carrying motion packets. */
    const val MOTION_UUID = "5af2dba6-9864-4524-812f-60c35dcb3ff0"

    /** Write characteristic carrying control commands. */
    const val CONTROL_UUID = "95978bed-ce1d-404c-932b-25cab3db5b2e"

    /** Size in bytes of a packet holding [samples] samples. */
    fun packetSize(samples: Int): Int = HEADER_SIZE + SAMPLE_SIZE * samples

    /**
     * How many samples fit into one notification when the negotiated ATT MTU is
     * [mtu], capped at [MAX_SAMPLES]. Zero means even one sample does not fit
     * (the default MTU of 23 is too small; the phone must negotiate a larger one).
     */
    fun samplesThatFit(mtu: Int): Int =
        ((mtu - ATT_OVERHEAD - HEADER_SIZE) / SAMPLE_SIZE).coerceIn(0, MAX_SAMPLES)
}
