package com.amsdots.android

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.os.Handler
import com.amsdots.stream.Reading
import com.amsdots.stream.SensorFuser

/**
 * Registers linear acceleration, gyroscope and gravity with the Android sensor
 * framework and feeds the readings to [fuser].
 *
 * Energy: the *wake-up* variant of each sensor is used when the phone has one,
 * with hardware batching of [MAX_REPORT_LATENCY_US], so the sensor hub collects
 * readings in its FIFO and the CPU wakes only about ten times a second. A
 * non-wake-up sensor stops delivering when the CPU suspends; [allWakeUp] tells
 * the service whether it must hold a wake lock instead.
 *
 * Events are delivered on [handler]'s thread.
 */
class MotionSensors(context: Context, private val handler: Handler, private val fuser: SensorFuser) {
    private val manager = context.getSystemService(SensorManager::class.java)

    private val linear = pick(Sensor.TYPE_LINEAR_ACCELERATION)
    private val gyroscope = pick(Sensor.TYPE_GYROSCOPE)
    private val gravity = pick(Sensor.TYPE_GRAVITY)

    private var registered = false

    /** Name of the first missing sensor, or `null` if the phone has all three. */
    val missing: String?
        get() = when {
            linear == null -> "linear acceleration sensor"
            gyroscope == null -> "gyroscope"
            gravity == null -> "gravity sensor"
            else -> null
        }

    /** `true` when all three sensors are wake-up sensors, so batching works with the CPU asleep. */
    val allWakeUp: Boolean
        get() = listOfNotNull(linear, gyroscope, gravity).all { it.isWakeUpSensor }

    /** One line per sensor, for the log. */
    fun describe(): String = listOf("linear" to linear, "gyro" to gyroscope, "gravity" to gravity).joinToString("; ") { (name, s) ->
        if (s == null) "$name: none" else "$name: ${s.name}${if (s.isWakeUpSensor) " (wake-up)" else ""}, fifo ${s.fifoMaxEventCount}, min ${s.minDelay}us"
    }

    /** Starts (or restarts at a new rate) delivery at about [rateHz]. */
    fun start(rateHz: Int) {
        stop()
        val periodUs = 1_000_000 / rateHz
        for (sensor in listOfNotNull(linear, gyroscope, gravity)) {
            // Gravity changes slowly; a quarter of the rate saves FIFO space and wakeups' work.
            val period = if (sensor === gravity) periodUs * 4 else periodUs
            manager.registerListener(listener, sensor, period, MAX_REPORT_LATENCY_US, handler)
        }
        registered = true
    }

    fun stop() {
        if (registered) manager.unregisterListener(listener)
        registered = false
    }

    private fun pick(type: Int): Sensor? = manager.getDefaultSensor(type, true) ?: manager.getDefaultSensor(type)

    private val listener = object : SensorEventListener {
        override fun onSensorChanged(event: SensorEvent) {
            val v = event.values
            val reading = Reading(event.timestamp, v[0], v[1], v[2])
            when (event.sensor.type) {
                Sensor.TYPE_LINEAR_ACCELERATION -> fuser.onLinearAcceleration(reading)
                Sensor.TYPE_GYROSCOPE -> fuser.onGyroscope(reading)
                Sensor.TYPE_GRAVITY -> fuser.onGravity(reading)
            }
        }

        override fun onAccuracyChanged(sensor: Sensor, accuracy: Int) = Unit
    }

    private companion object {
        /** How long the sensor hub may hold readings before waking the CPU: about four samples at 50 Hz. */
        const val MAX_REPORT_LATENCY_US = 80_000
    }
}
