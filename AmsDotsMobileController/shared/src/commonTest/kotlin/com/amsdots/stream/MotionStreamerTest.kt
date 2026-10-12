package com.amsdots.stream

import com.amsdots.protocol.MotionSample
import com.amsdots.protocol.PacketCodec
import com.amsdots.protocol.Vec3
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class MotionStreamerTest {
    private class FakeSink : NotificationSink {
        val sent = mutableListOf<ByteArray>()
        var accept = true
        override fun send(bytes: ByteArray): Boolean {
            if (accept) sent += bytes
            return accept
        }
    }

    private val sink = FakeSink()
    private val streamer = MotionStreamer(sink, maxLatencyMs = 100, maxBacklog = 8)

    private fun sample(tMs: Long, marker: Int = 0) =
        FusedSample(tMs * 1_000_000, MotionSample(Vec3(marker, 0, 0), Vec3(0, 0, 0)), Vec3(0, 0, 1000))

    /** MTU 64 fits four samples (13 + 48 + 3). */
    private val bigMtu = 64

    @Test
    fun sendsNothingUntilAnMtuIsKnown() {
        repeat(4) { streamer.onSample(sample(it * 20L)) }
        assertTrue(sink.sent.isEmpty())
        streamer.onMtu(bigMtu)
        assertEquals(1, sink.sent.size)
    }

    @Test
    fun waitsForAFullPacketThenSendsFourSamples() {
        streamer.onMtu(bigMtu)
        repeat(3) { streamer.onSample(sample(it * 20L)) }
        assertTrue(sink.sent.isEmpty())
        streamer.onSample(sample(60))
        val packet = PacketCodec.decode(sink.sent.single())
        assertEquals(4, packet.samples.size)
        assertEquals(0, packet.sequence)
        assertEquals(0, packet.firstSampleMs)
    }

    @Test
    fun aSmallMtuMeansFewerSamplesPerPacket() {
        streamer.onMtu(28) // one sample
        streamer.onSample(sample(0))
        assertEquals(1, PacketCodec.decode(sink.sent.single()).samples.size)
    }

    @Test
    fun oldSamplesAreFlushedAsAPartialPacket() {
        streamer.onMtu(bigMtu)
        streamer.onSample(sample(0))
        streamer.onSample(sample(50))
        assertTrue(sink.sent.isEmpty())
        streamer.onSample(sample(120)) // the oldest has waited more than 100 ms
        assertEquals(3, PacketCodec.decode(sink.sent.single()).samples.size)
    }

    @Test
    fun onlyOneNotificationIsInFlight() {
        streamer.onMtu(bigMtu)
        repeat(8) { streamer.onSample(sample(it * 20L, marker = it)) }
        assertEquals(1, sink.sent.size)
        streamer.onNotificationSent()
        assertEquals(2, sink.sent.size)
        val second = PacketCodec.decode(sink.sent[1])
        assertEquals(1, second.sequence)
        assertEquals(listOf(4, 5, 6, 7), second.samples.map { it.acceleration.x })
        assertEquals(80, second.firstSampleMs)
    }

    @Test
    fun aSlowLinkDropsTheOldestSamples() {
        streamer.onMtu(bigMtu)
        repeat(4) { streamer.onSample(sample(it * 20L, marker = it)) } // in flight
        repeat(12) { streamer.onSample(sample(100 + it * 20L, marker = 100 + it)) }
        assertEquals(4L, streamer.samplesDropped) // 12 queued, backlog limit 8
        streamer.onNotificationSent()
        assertEquals(listOf(104, 105, 106, 107), PacketCodec.decode(sink.sent[1]).samples.map { it.acceleration.x })
    }

    @Test
    fun aFailedSendDropsThoseSamplesAndDoesNotBlock() {
        streamer.onMtu(bigMtu)
        sink.accept = false
        repeat(4) { streamer.onSample(sample(it * 20L)) }
        assertEquals(4L, streamer.samplesDropped)
        sink.accept = true
        repeat(4) { streamer.onSample(sample(100 + it * 20L)) }
        assertEquals(1, sink.sent.size)
    }

    @Test
    fun sequenceCountsUpAndResetRestartsIt() {
        streamer.onMtu(bigMtu)
        repeat(3) {
            repeat(4) { i -> streamer.onSample(sample(it * 100L + i)) }
            streamer.onNotificationSent()
        }
        assertEquals(listOf(0, 1, 2), sink.sent.map { PacketCodec.decode(it).sequence })
        streamer.reset()
        streamer.onMtu(bigMtu)
        repeat(4) { streamer.onSample(sample(5000L + it)) }
        val after = PacketCodec.decode(sink.sent.last())
        assertEquals(0, after.sequence)
        assertEquals(0, after.firstSampleMs) // the clock restarts with the new subscription
    }
}
