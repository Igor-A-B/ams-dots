package com.amsdots.stream

import kotlin.math.sqrt

/** Achieved sample rate over a window and how uneven the spacing was. */
data class RateReport(val samples: Int, val rateHz: Double, val jitterMs: Double, val maxGapMs: Double)

/**
 * Measures the rate a sensor stream really delivers (M2: "log achieved sample
 * rate and jitter"). Jitter is the standard deviation of the gaps between
 * consecutive sample times; with hardware batching the *sensor* timestamps
 * stay even even though delivery is bursty, which is what the laptop sees.
 */
class RateMeter {
    private var firstNs = 0L
    private var lastNs = 0L
    private var count = 0
    private var sumGap = 0.0
    private var sumGapSq = 0.0
    private var maxGap = 0.0

    fun add(timeNs: Long) {
        if (count > 0) {
            val gapMs = (timeNs - lastNs) / 1e6
            sumGap += gapMs
            sumGapSq += gapMs * gapMs
            if (gapMs > maxGap) maxGap = gapMs
        } else {
            firstNs = timeNs
        }
        lastNs = timeNs
        count++
    }

    /** Report for everything added since the last [reset]; `null` with fewer than two samples. */
    fun report(): RateReport? {
        if (count < 2) return null
        val gaps = count - 1
        val mean = sumGap / gaps
        val variance = (sumGapSq / gaps - mean * mean).coerceAtLeast(0.0)
        val spanS = (lastNs - firstNs) / 1e9
        return RateReport(count, if (spanS > 0) gaps / spanS else 0.0, sqrt(variance), maxGap)
    }

    fun reset() {
        count = 0
        sumGap = 0.0
        sumGapSq = 0.0
        maxGap = 0.0
    }
}
