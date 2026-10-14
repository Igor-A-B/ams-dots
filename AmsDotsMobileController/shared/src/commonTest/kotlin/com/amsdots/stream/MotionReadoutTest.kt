package com.amsdots.stream

import kotlin.math.abs
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class MotionReadoutTest {
    private fun near(expected: Double, actual: Double?, tolerance: Double = 0.01) =
        assertTrue(actual != null && abs(expected - actual) < tolerance, "expected $expected, got $actual")

    private val flat = doubleArrayOf(0.0, 0.0, 1000.0)
    private val upright = doubleArrayOf(0.0, 1000.0, 0.0)
    private val still = doubleArrayOf(0.0, 0.0, 0.0)

    @Test
    fun splitsAccelerationByGravityWhateverThePose() {
        val m = ReadoutMeter()
        m.add(doubleArrayOf(0.0, 2.0, 1.0), still, flat)
        val r = m.report()!!
        near(2.0, r.horizontal)
        near(1.0, r.vertical)
        near(0.0, r.tiltDegrees)

        m.reset()
        m.add(doubleArrayOf(0.0, 1.0, -2.0), still, upright)
        val u = m.report()!!
        near(2.0, u.horizontal)
        near(1.0, u.vertical)
        near(90.0, u.tiltDegrees)
    }

    @Test
    fun vibrationAveragesOutButThePeakIsKept() {
        val m = ReadoutMeter()
        repeat(10) { m.add(doubleArrayOf(if (it % 2 == 0) 3.0 else -3.0, 0.0, 0.0), still, flat) }
        val r = m.report()!!
        near(0.0, r.horizontal)
        near(3.0, r.horizontalPeak)
        assertEquals(10, r.samples)
    }

    @Test
    fun turningIsRotationAboutUp() {
        val m = ReadoutMeter()
        m.add(still, doubleArrayOf(0.0, 0.5, 0.2), upright)
        val r = m.report()!!
        near(0.5, r.yaw)
        near(kotlin.math.sqrt(0.29), r.rotationPeak)
    }

    @Test
    fun nothingToReportWithoutSamples() {
        assertNull(ReadoutMeter().report())
    }

    @Test
    fun gpsAccelerationIsTheChangeOfSpeedBetweenFixes() {
        val m = SpeedMeter()
        assertNull(m.add(0, 10.0, 5.0).acceleration)
        near(2.0, m.add(1_000_000_000, 12.0, 5.0).acceleration)
        assertNull(m.add(20_000_000_000, 12.0, 5.0).acceleration, "a long gap says nothing about acceleration")
    }
}
