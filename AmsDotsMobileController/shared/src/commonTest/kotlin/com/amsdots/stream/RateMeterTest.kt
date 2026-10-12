package com.amsdots.stream

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull

class RateMeterTest {
    @Test
    fun needsTwoSamples() {
        val meter = RateMeter()
        assertNull(meter.report())
        meter.add(0)
        assertNull(meter.report())
    }

    @Test
    fun evenSamplesGiveTheRateAndNoJitter() {
        val meter = RateMeter()
        repeat(51) { meter.add(it * 20_000_000L) } // 50 Hz for one second
        val report = assertNotNull(meter.report())
        assertEquals(51, report.samples)
        assertEquals(50.0, report.rateHz, 1e-6)
        assertEquals(0.0, report.jitterMs, 1e-6)
        assertEquals(20.0, report.maxGapMs, 1e-6)
    }

    @Test
    fun unevenSamplesShowJitterAndTheLargestGap() {
        val meter = RateMeter()
        for (t in listOf(0L, 10, 30, 40, 60)) meter.add(t * 1_000_000) // gaps 10, 20, 10, 20
        val report = assertNotNull(meter.report())
        assertEquals(5.0, report.jitterMs, 1e-6)
        assertEquals(20.0, report.maxGapMs, 1e-6)
    }

    @Test
    fun resetStartsANewWindow() {
        val meter = RateMeter()
        meter.add(0)
        meter.add(1_000_000_000)
        meter.reset()
        assertNull(meter.report())
        meter.add(5_000_000_000)
        meter.add(5_100_000_000)
        assertEquals(10.0, assertNotNull(meter.report()).rateHz, 1e-6)
    }
}
