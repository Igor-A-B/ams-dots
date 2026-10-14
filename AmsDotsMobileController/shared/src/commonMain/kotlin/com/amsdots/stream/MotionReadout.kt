package com.amsdots.stream

import kotlin.math.PI
import kotlin.math.acos
import kotlin.math.max
import kotlin.math.sqrt

/**
 * Debug figures for the screen, over the last window of samples (see [ReadoutMeter]). Split by gravity, so they
 * do not depend on how the phone is mounted: "horizontal" is what moves the laptop's dots, "vertical" is bumps.
 *
 * @property horizontal length of the average horizontal acceleration, m/s² (vibration averages out)
 * @property horizontalPeak largest single horizontal acceleration in the window, m/s²
 * @property vertical average acceleration along gravity, m/s² (positive = up)
 * @property yaw average rotation about the vertical, rad/s (positive = turning left); what the dots count as turning
 * @property rotationPeak largest total rotation rate in the window, rad/s, whatever the axis
 * @property tiltDegrees angle between the screen's normal and up: 0 = lying flat face up, 90 = upright
 * @property samples samples in the window; 0 means the sensors deliver nothing
 */
data class MotionReadout(
    val horizontal: Double,
    val horizontalPeak: Double,
    val vertical: Double,
    val yaw: Double,
    val rotationPeak: Double,
    val tiltDegrees: Double,
    val samples: Int,
)

/** Speed from GPS, the only speed the phone can know. [acceleration] is the change of speed between fixes, m/s². */
data class SpeedReadout(val metersPerSecond: Double, val accuracyMeters: Double?, val acceleration: Double?)

/** Collects samples and turns them into a [MotionReadout] on demand. Not thread-safe. */
class ReadoutMeter {
    private var count = 0
    private val sumHorizontal = DoubleArray(3)
    private var peakHorizontal = 0.0
    private var sumVertical = 0.0
    private var sumYaw = 0.0
    private var peakRotation = 0.0
    private var lastUp: DoubleArray? = null

    /** [accel] in m/s² and [gyro] in rad/s in the phone's axes, [gravity] in any unit (only its direction is used). */
    fun add(accel: DoubleArray, gyro: DoubleArray, gravity: DoubleArray) {
        val g = norm(gravity)
        if (g < 1e-6) return
        val up = DoubleArray(3) { gravity[it] / g }
        val along = dot(accel, up)
        val horizontal = DoubleArray(3) { accel[it] - along * up[it] }
        for (i in 0..2) sumHorizontal[i] += horizontal[i]
        peakHorizontal = max(peakHorizontal, norm(horizontal))
        sumVertical += along
        sumYaw += dot(gyro, up)
        peakRotation = max(peakRotation, norm(gyro))
        lastUp = up
        count++
    }

    /** The figures since the last [reset], or `null` before any sample. */
    fun report(): MotionReadout? {
        val up = lastUp ?: return null
        val n = count.toDouble()
        return MotionReadout(
            horizontal = norm(DoubleArray(3) { sumHorizontal[it] / n }),
            horizontalPeak = peakHorizontal,
            vertical = sumVertical / n,
            yaw = sumYaw / n,
            rotationPeak = peakRotation,
            tiltDegrees = acos(up[2].coerceIn(-1.0, 1.0)) * 180.0 / PI,
            samples = count,
        )
    }

    fun reset() {
        count = 0
        sumHorizontal.fill(0.0)
        peakHorizontal = 0.0
        sumVertical = 0.0
        sumYaw = 0.0
        peakRotation = 0.0
        lastUp = null
    }

    private fun dot(a: DoubleArray, b: DoubleArray) = a[0] * b[0] + a[1] * b[1] + a[2] * b[2]

    private fun norm(a: DoubleArray) = sqrt(dot(a, a))
}

/** Turns successive GPS fixes into a [SpeedReadout]. Not thread-safe. */
class SpeedMeter {
    private var previous: Pair<Long, Double>? = null

    /** A fix at [timeNs] (elapsed-realtime clock) with [speed] in m/s. */
    fun add(timeNs: Long, speed: Double, accuracyMeters: Double?): SpeedReadout {
        val before = previous
        previous = timeNs to speed
        val seconds = before?.let { (timeNs - it.first) / 1e9 }
        val acceleration = if (before != null && seconds != null && seconds in 0.2..5.0) (speed - before.second) / seconds else null
        return SpeedReadout(speed, accuracyMeters, acceleration)
    }

    fun reset() {
        previous = null
    }
}
