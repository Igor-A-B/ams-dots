package com.amsdots.stream

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class SensorFuserTest {
    private val out = mutableListOf<FusedSample>()
    private val fuser = SensorFuser { out += it }

    private fun ms(n: Long) = n * 1_000_000

    private fun gyro(tMs: Long, x: Float = 0f) = Reading(ms(tMs), x, 0f, 0f)
    private fun accel(tMs: Long, x: Float) = Reading(ms(tMs), x, 0f, 0f)
    private fun gravity() = Reading(0, 0f, 0f, 9.81f)

    @Test
    fun nothingIsEmittedWithoutGravity() {
        fuser.onLinearAcceleration(accel(0, 1f))
        fuser.onGyroscope(gyro(0))
        assertTrue(out.isEmpty())
        fuser.onGravity(gravity())
        assertEquals(1, out.size)
    }

    @Test
    fun gyroscopeWaitsForAccelerationToCatchUp() {
        fuser.onGravity(gravity())
        fuser.onGyroscope(gyro(20))
        fuser.onLinearAcceleration(accel(0, 0f))
        assertTrue(out.isEmpty())
        fuser.onLinearAcceleration(accel(40, 4f))
        assertEquals(1, out.size)
    }

    @Test
    fun accelerationIsInterpolatedAtTheGyroscopeTime() {
        fuser.onGravity(gravity())
        fuser.onLinearAcceleration(accel(0, 0f))
        fuser.onLinearAcceleration(accel(40, 4f))
        fuser.onGyroscope(gyro(10))
        // 10 ms into a 40 ms span from 0 to 4 m/s² is 1 m/s² = 1000 mm/s²
        assertEquals(1000, out.single().sample.acceleration.x)
        assertEquals(ms(10), out.single().timeNs)
    }

    @Test
    fun burstsDeliveredInDifferentOrderStillPair() {
        fuser.onGravity(gravity())
        // the gyroscope batch arrives first, then the acceleration batch
        for (t in listOf(0L, 20, 40)) fuser.onGyroscope(gyro(t, x = t / 10f))
        for (t in listOf(0L, 20, 40)) fuser.onLinearAcceleration(accel(t, t / 10f))
        assertEquals(listOf(0, 2000, 4000), out.map { it.sample.acceleration.x })
        assertEquals(listOf(0, 2000, 4000), out.map { it.sample.rotation.x })
    }

    @Test
    fun gravityIsReportedInThousandthsOfG() {
        fuser.onGravity(Reading(0, 0f, 4.905f, 9.81f))
        fuser.onLinearAcceleration(accel(0, 0f))
        fuser.onGyroscope(gyro(0))
        val g = out.single().gravityMg
        assertEquals(0, g.x)
        assertEquals(500, g.y)
        assertEquals(1000, g.z)
    }

    @Test
    fun resetDropsEverythingBuffered() {
        fuser.onGravity(gravity())
        fuser.onGyroscope(gyro(20))
        fuser.reset()
        fuser.onGravity(gravity())
        fuser.onLinearAcceleration(accel(40, 1f))
        assertTrue(out.isEmpty())
    }
}
