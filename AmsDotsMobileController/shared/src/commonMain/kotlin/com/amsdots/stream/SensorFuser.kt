package com.amsdots.stream

import com.amsdots.protocol.MotionSample
import com.amsdots.protocol.Vec3

/** One 3-axis sensor event; [timeNs] is the sensor clock in nanoseconds. */
class Reading(val timeNs: Long, val x: Float, val y: Float, val z: Float)

/** A fused sample ready for a packet: values in the phone's axes plus gravity in 1/1000 g. */
class FusedSample(val timeNs: Long, val sample: MotionSample, val gravityMg: Vec3)

/**
 * Combines the three separate Android sensor streams into samples.
 *
 * With hardware batching each sensor delivers its events in bursts at its own
 * moments, so "the latest value of each sensor" can be up to a whole batch
 * out of date. Instead one sample is produced per gyroscope event, at the
 * gyroscope's own time: linear acceleration is interpolated between its two
 * events around that time (a sample waits until the acceleration stream has
 * caught up), and gravity, which changes slowly, is the latest reading.
 *
 * Not thread-safe: feed it from a single thread.
 */
class SensorFuser(private val emit: (FusedSample) -> Unit) {
    private val pendingGyro = ArrayDeque<Reading>()
    private val accelHistory = ArrayDeque<Reading>()
    private var gravity: Reading? = null

    fun onGyroscope(reading: Reading) {
        pendingGyro.addLast(reading)
        if (pendingGyro.size > MAX_PENDING) pendingGyro.removeFirst()
        drain()
    }

    fun onLinearAcceleration(reading: Reading) {
        accelHistory.addLast(reading)
        drain()
    }

    fun onGravity(reading: Reading) {
        gravity = reading
        drain()
    }

    /** Forgets everything buffered, e.g. when the sensors are restarted. */
    fun reset() {
        pendingGyro.clear()
        accelHistory.clear()
        gravity = null
    }

    private fun drain() {
        val g = gravity ?: return
        while (pendingGyro.isNotEmpty()) {
            val gyro = pendingGyro.first()
            val newest = accelHistory.lastOrNull() ?: return
            if (gyro.timeNs > newest.timeNs) return // acceleration has not caught up yet
            pendingGyro.removeFirst()
            val accel = accelAt(gyro.timeNs)
            emit(
                FusedSample(
                    timeNs = gyro.timeNs,
                    sample = MotionSample.fromSensors(floatArrayOf(accel.first, accel.second, accel.third), floatArrayOf(gyro.x, gyro.y, gyro.z)),
                    gravityMg = Vec3(milliG(g.x), milliG(g.y), milliG(g.z)),
                ),
            )
        }
        // keep only the history a pending or future gyro event can still need
        val needFrom = pendingGyro.firstOrNull()?.timeNs
        while (accelHistory.size > 2 && (needFrom == null || accelHistory[1].timeNs <= needFrom)) accelHistory.removeFirst()
    }

    /** Linear acceleration at [timeNs]; the history is non-empty and its newest event is not older than [timeNs]. */
    private fun accelAt(timeNs: Long): Triple<Float, Float, Float> {
        var before: Reading? = null
        for (r in accelHistory) {
            if (r.timeNs >= timeNs) {
                val a = before ?: return Triple(r.x, r.y, r.z)
                val span = r.timeNs - a.timeNs
                if (span <= 0) return Triple(r.x, r.y, r.z)
                val f = (timeNs - a.timeNs).toFloat() / span.toFloat()
                return Triple(a.x + (r.x - a.x) * f, a.y + (r.y - a.y) * f, a.z + (r.z - a.z) * f)
            }
            before = r
        }
        val last = accelHistory.last()
        return Triple(last.x, last.y, last.z)
    }

    private fun milliG(metersPerSecond2: Float): Int =
        (metersPerSecond2 / GRAVITY_EARTH * 1000f).toInt().coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt())

    private companion object {
        /** Bound on gyro events waiting for acceleration, so a dead sensor cannot grow memory. */
        const val MAX_PENDING = 200

        /** The protocol sends gravity in 1/1000 of this (see `docs/protocol.md`). */
        const val GRAVITY_EARTH = 9.81f
    }
}
