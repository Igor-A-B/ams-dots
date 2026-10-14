package com.amsdots.stream

/** Where the phone side currently is, from idle to streaming. */
enum class LinkPhase {
    /** Service not running; nothing is advertised and no sensor is on. */
    STOPPED,

    /** Advertising; waiting for the laptop. Only the slow advertising costs energy. */
    ADVERTISING,

    /** The laptop is connected but has not subscribed to motion yet; sensors are off. */
    CONNECTED,

    /** The laptop is subscribed; sensors are on and packets flow. */
    STREAMING,

    /** Sensors only, no Bluetooth (test mode for measuring the sensors alone). */
    SENSOR_TEST,

    /** Something prevented running; see [LinkStatus.error]. */
    ERROR,
}

/**
 * Everything the screen shows.
 *
 * @property targetRateHz sample rate currently requested of the sensors
 * @property measured achieved rate and jitter of the last window, if sensors are on
 * @property packetsSent notifications sent since the laptop subscribed
 * @property readout acceleration and rotation over the last fraction of a second, while the sensors run
 * @property speed latest GPS speed, while the service runs and location is allowed
 */
data class LinkStatus(
    val phase: LinkPhase = LinkPhase.STOPPED,
    val targetRateHz: Int = DEFAULT_RATE_HZ,
    val measured: RateReport? = null,
    val packetsSent: Long = 0,
    val error: String? = null,
    val readout: MotionReadout? = null,
    val speed: SpeedReadout? = null,
) {
    val running: Boolean get() = phase != LinkPhase.STOPPED && phase != LinkPhase.ERROR

    companion object {
        /** Below the 200 Hz limit that needs an extra permission; plenty for vehicle motion. */
        const val DEFAULT_RATE_HZ = 50
    }
}
