package com.amsdots.stream

import com.amsdots.protocol.MotionPacket
import com.amsdots.protocol.PacketCodec
import com.amsdots.protocol.Protocol
import com.amsdots.protocol.SpeedFix
import kotlin.math.roundToInt

/** Where encoded packets go: the BLE notification of the motion characteristic. */
interface NotificationSink {
    /**
     * Starts sending [bytes] as one notification. Returns `false` if it could not be
     * started. After `true`, the sink calls [MotionStreamer.onNotificationSent] once the
     * stack is ready for the next one.
     */
    fun send(bytes: ByteArray): Boolean
}

/**
 * Turns fused samples into packets and paces them out, one notification in
 * flight at a time (BLE drops a notification sent before the previous one
 * completed).
 *
 * Samples wait until a packet is full for the negotiated MTU, or until the
 * oldest has waited [maxLatencyMs]. If the link cannot keep up the oldest
 * samples are dropped, so the laptop always sees recent motion rather than
 * a growing backlog.
 *
 * The newest GPS speed ([onSpeed]) rides along in every packet until it is older than
 * [Protocol.MAX_FIX_AGE_MS], after which packets say "no speed".
 *
 * Not thread-safe: call it from a single thread.
 */
class MotionStreamer(
    private val sink: NotificationSink,
    private val maxLatencyMs: Long = 60,
    private val maxBacklog: Int = 50,
) {
    private val queue = ArrayDeque<FusedSample>()
    private var sequence = 0
    private var startNs: Long? = null
    private var inFlight = false

    /** The newest GPS fix: time on the sensors' clock (ns) and speed (m/s). */
    private var fix: Pair<Long, Double>? = null

    /** Largest sample count per packet for the current MTU; 0 until an MTU is known. */
    private var perPacket = 0

    var packetsSent = 0L
        private set
    var samplesDropped = 0L
        private set

    /** Call when the central connects or its MTU changes. */
    fun onMtu(mtu: Int) {
        perPacket = Protocol.samplesThatFit(mtu)
        pump()
    }

    fun onSample(sample: FusedSample) {
        if (startNs == null) startNs = sample.timeNs
        queue.addLast(sample)
        while (queue.size > maxBacklog) {
            queue.removeFirst()
            samplesDropped++
        }
        pump()
    }

    /** A GPS fix: [speed] in m/s at [timeNs], on the same clock as the samples' times. */
    fun onSpeed(timeNs: Long, speed: Double) {
        if (speed.isFinite() && speed >= 0) fix = timeNs to speed
    }

    fun onNotificationSent() {
        inFlight = false
        pump()
    }

    /** Forget queued samples and the link state: the central unsubscribed or disconnected. */
    fun reset() {
        queue.clear()
        inFlight = false
        perPacket = 0
        startNs = null
        sequence = 0
    }

    private fun pump() {
        if (inFlight || perPacket == 0) return
        val first = queue.firstOrNull() ?: return
        val newest = queue.last()
        val waitedMs = (newest.timeNs - first.timeNs) / 1_000_000
        if (queue.size < perPacket && waitedMs < maxLatencyMs) return

        val count = minOf(queue.size, perPacket)
        val samples = List(count) { queue.removeFirst() }
        val packet = MotionPacket(
            sequence = sequence,
            firstSampleMs = (samples[0].timeNs - (startNs ?: samples[0].timeNs)) / 1_000_000 and 0xFFFF_FFFFL,
            gravityMg = samples.last().gravityMg,
            samples = samples.map { it.sample },
            speed = speedFor(samples[0].timeNs),
        )
        if (sink.send(PacketCodec.encode(packet))) {
            inFlight = true
            sequence = (sequence + 1) and 0xFFFF
            packetsSent++
        } else {
            samplesDropped += count
        }
    }

    /** The fix as the packet whose first sample is at [firstSampleNs] carries it, or `null` if there is none or it is too old. */
    private fun speedFor(firstSampleNs: Long): SpeedFix? {
        val (fixNs, speed) = fix ?: return null
        val ageMs = ((firstSampleNs - fixNs) / 1_000_000).coerceIn(Short.MIN_VALUE.toLong(), Short.MAX_VALUE.toLong()).toInt()
        if (ageMs > Protocol.MAX_FIX_AGE_MS) return null
        return SpeedFix((speed * 100).roundToInt().coerceIn(0, Protocol.MAX_SPEED_CM_S), ageMs)
    }
}
