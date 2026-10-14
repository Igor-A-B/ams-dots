package com.amsdots.protocol

/**
 * Converts [MotionPacket] to and from the bytes of a BLE notification. All
 * numbers are little-endian; the layout is in `docs/protocol.md`.
 */
object PacketCodec {

    /** Encodes [packet]. Throws [IllegalArgumentException] if a field does not fit its wire size. */
    fun encode(packet: MotionPacket): ByteArray {
        require(packet.samples.size in 1..Protocol.MAX_SAMPLES) { "a packet holds 1..${Protocol.MAX_SAMPLES} samples" }
        require(packet.sequence in 0..0xFFFF) { "sequence must fit 16 bits" }
        require(packet.firstSampleMs in 0..0xFFFF_FFFFL) { "time must fit 32 bits" }
        packet.speed?.let {
            require(it.centimetersPerSecond in 0..Protocol.MAX_SPEED_CM_S) { "speed must fit 16 bits" }
            require(it.ageMs in SHORT) { "speed age must fit 16 bits" }
        }

        val out = ByteArray(Protocol.packetSize(packet.samples.size))
        var at = 0
        out[at++] = Protocol.VERSION.toByte()
        at = putU16(out, at, packet.sequence)
        at = putU32(out, at, packet.firstSampleMs)
        at = putVec(out, at, packet.gravityMg)
        at = putU16(out, at, packet.speed?.centimetersPerSecond ?: Protocol.NO_SPEED)
        at = putU16(out, at, packet.speed?.ageMs ?: 0)
        for (sample in packet.samples) {
            at = putVec(out, at, sample.acceleration)
            at = putVec(out, at, sample.rotation)
        }
        return out
    }

    /** Decodes [bytes] of either version. Throws [PacketFormatException] if they are not a valid packet. */
    fun decode(bytes: ByteArray): MotionPacket {
        val version = if (bytes.isEmpty()) 0 else bytes[0].toInt() and 0xFF
        // The version decides the header length, so a missing byte and an unknown version are told apart first.
        val headerSize = when (version) {
            Protocol.VERSION_1 -> Protocol.HEADER_SIZE_V1
            Protocol.VERSION -> Protocol.HEADER_SIZE
            else -> if (bytes.isEmpty()) Protocol.HEADER_SIZE_V1 else throw PacketFormatException(PacketError.BAD_VERSION, "unsupported version $version")
        }
        if (bytes.size < headerSize) {
            throw PacketFormatException(PacketError.TOO_SHORT, "packet has ${bytes.size} bytes, header needs $headerSize")
        }
        val sampleBytes = bytes.size - headerSize
        if (sampleBytes % Protocol.SAMPLE_SIZE != 0) {
            throw PacketFormatException(PacketError.BAD_LENGTH, "$sampleBytes sample bytes is not a multiple of ${Protocol.SAMPLE_SIZE}")
        }
        val count = sampleBytes / Protocol.SAMPLE_SIZE
        if (count !in 1..Protocol.MAX_SAMPLES) {
            throw PacketFormatException(PacketError.BAD_COUNT, "$count samples, expected 1..${Protocol.MAX_SAMPLES}")
        }

        val sequence = getU16(bytes, 1)
        val firstSampleMs = getU32(bytes, 3)
        val gravity = getVec(bytes, 7)
        val speed = if (version == Protocol.VERSION) {
            getU16(bytes, 13).takeIf { it != Protocol.NO_SPEED }?.let { SpeedFix(it, getU16(bytes, 15).toShort().toInt()) }
        } else {
            null
        }
        val samples = List(count) { i ->
            val base = headerSize + i * Protocol.SAMPLE_SIZE
            MotionSample(getVec(bytes, base), getVec(bytes, base + 6))
        }
        return MotionPacket(sequence, firstSampleMs, gravity, samples, speed)
    }

    // --- little-endian helpers; each put returns the next write position ---

    private fun putU16(out: ByteArray, at: Int, value: Int): Int {
        out[at] = value.toByte()
        out[at + 1] = (value shr 8).toByte()
        return at + 2
    }

    private fun putU32(out: ByteArray, at: Int, value: Long): Int {
        for (i in 0 until 4) out[at + i] = (value shr (8 * i)).toByte()
        return at + 4
    }

    private fun putVec(out: ByteArray, at: Int, v: Vec3): Int {
        require(v.x in SHORT && v.y in SHORT && v.z in SHORT) { "value outside 16-bit range: $v" }
        return putU16(out, putU16(out, putU16(out, at, v.x), v.y), v.z)
    }

    private fun getU16(bytes: ByteArray, at: Int): Int =
        (bytes[at].toInt() and 0xFF) or ((bytes[at + 1].toInt() and 0xFF) shl 8)

    private fun getU32(bytes: ByteArray, at: Int): Long =
        (getU16(bytes, at).toLong()) or (getU16(bytes, at + 2).toLong() shl 16)

    /** Reads three signed 16-bit values. `toShort` reinterprets the unsigned read as signed. */
    private fun getVec(bytes: ByteArray, at: Int): Vec3 = Vec3(
        getU16(bytes, at).toShort().toInt(),
        getU16(bytes, at + 2).toShort().toInt(),
        getU16(bytes, at + 4).toShort().toInt(),
    )

    private val SHORT = Short.MIN_VALUE.toInt()..Short.MAX_VALUE.toInt()
}
