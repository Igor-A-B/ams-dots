package com.amsdots.protocol

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull

class PacketCodecTest {
    private fun sample(n: Int) = MotionSample(Vec3(n, -n, n * 2), Vec3(-n * 3, n, 0))

    private fun packet(samples: Int, sequence: Int = 7, time: Long = 1000) =
        MotionPacket(sequence, time, Vec3(1, -2, 1000), List(samples) { sample(it + 1) })

    @Test
    fun roundTripKeepsEveryField() {
        for (count in 1..Protocol.MAX_SAMPLES) {
            val original = packet(count)
            assertEquals(original, PacketCodec.decode(PacketCodec.encode(original)))
        }
    }

    @Test
    fun encodedSizeMatchesTheSpecification() {
        assertEquals(25, PacketCodec.encode(packet(1)).size)
        assertEquals(61, PacketCodec.encode(packet(4)).size)
    }

    @Test
    fun numbersAreLittleEndianAndSigned() {
        val bytes = PacketCodec.encode(
            MotionPacket(0x0201, 0x0A0B0C0DL, Vec3(-1, 0, 0), listOf(MotionSample(Vec3(0x1234, 0, 0), Vec3(0, 0, 0)))),
        )
        assertEquals(1, bytes[0].toInt())
        assertContentEquals(byteArrayOf(0x01, 0x02), bytes.copyOfRange(1, 3))
        assertContentEquals(byteArrayOf(0x0D, 0x0C, 0x0B, 0x0A), bytes.copyOfRange(3, 7))
        assertContentEquals(byteArrayOf(0xFF.toByte(), 0xFF.toByte()), bytes.copyOfRange(7, 9))
        assertContentEquals(byteArrayOf(0x34, 0x12), bytes.copyOfRange(13, 15))
    }

    @Test
    fun encodeRejectsValuesThatDoNotFit() {
        assertFailsWith<IllegalArgumentException> { PacketCodec.encode(packet(0)) }
        assertFailsWith<IllegalArgumentException> { PacketCodec.encode(packet(5)) }
        assertFailsWith<IllegalArgumentException> { PacketCodec.encode(packet(1, sequence = 65536)) }
        assertFailsWith<IllegalArgumentException> { PacketCodec.encode(packet(1, time = 1L shl 32)) }
        assertFailsWith<IllegalArgumentException> {
            PacketCodec.encode(MotionPacket(0, 0, Vec3(40000, 0, 0), listOf(sample(1))))
        }
    }

    @Test
    fun decodeReportsWhatIsWrong() {
        val good = PacketCodec.encode(packet(1))
        fun errorOf(bytes: ByteArray) = assertFailsWith<PacketFormatException> { PacketCodec.decode(bytes) }.error

        assertEquals(PacketError.TOO_SHORT, errorOf(ByteArray(0)))
        assertEquals(PacketError.TOO_SHORT, errorOf(good.copyOf(12)))
        assertEquals(PacketError.BAD_COUNT, errorOf(good.copyOf(13)))
        assertEquals(PacketError.BAD_LENGTH, errorOf(good.copyOf(good.size + 3)))
        assertEquals(PacketError.BAD_VERSION, errorOf(good.copyOf().also { it[0] = 2 }))
        assertEquals(PacketError.BAD_COUNT, errorOf(ByteArray(Protocol.packetSize(5)).also { it[0] = 1 }))
    }

    @Test
    fun sensorValuesAreConvertedAndSaturated() {
        val s = MotionSample.fromSensors(floatArrayOf(1.5f, -2.25f, 100f), floatArrayOf(0.001f, -0.5f, -100f))
        assertEquals(Vec3(1500, -2250, 32767), s.acceleration)
        assertEquals(Vec3(1, -500, -32768), s.rotation)
    }

    @Test
    fun mtuDecidesHowManySamplesFit() {
        assertEquals(0, Protocol.samplesThatFit(23))
        assertEquals(1, Protocol.samplesThatFit(28))
        assertEquals(3, Protocol.samplesThatFit(63))
        assertEquals(4, Protocol.samplesThatFit(64))
        assertEquals(4, Protocol.samplesThatFit(517))
    }

    @Test
    fun controlCommandsRoundTripAndBadInputIsIgnored() {
        val bytes = ControlCommand.encode(ControlCommand.SetRate(50))
        assertContentEquals(byteArrayOf(0x01, 50), bytes)
        assertEquals(ControlCommand.SetRate(50), ControlCommand.decode(bytes))
        assertNull(ControlCommand.decode(byteArrayOf(0x01, 5)), "rate below minimum")
        assertNull(ControlCommand.decode(byteArrayOf(0x01, 101)), "rate above maximum")
        assertNull(ControlCommand.decode(byteArrayOf(0x7F, 50)), "unknown command")
        assertNull(ControlCommand.decode(byteArrayOf(0x01)), "too short")
        assertFailsWith<IllegalArgumentException> { ControlCommand.SetRate(500) }
    }
}
