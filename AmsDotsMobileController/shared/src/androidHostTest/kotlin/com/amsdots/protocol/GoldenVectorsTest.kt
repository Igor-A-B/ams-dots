package com.amsdots.protocol

import java.io.File
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/**
 * Checks the codec against `protocol/vectors.txt`, which was written by an
 * independent encoder. Lives in the host-test source set because it reads a file.
 */
class GoldenVectorsTest {
    private class Block(val kind: String, val name: String, val fields: List<Pair<String, String>>) {
        fun one(key: String) = fields.first { it.first == key }.second
        fun all(key: String) = fields.filter { it.first == key }.map { it.second }
    }

    private fun load(): List<Block> {
        // Gradle runs host tests with the module directory (shared/) as working directory.
        val file = listOf(File("../protocol/vectors.txt"), File("protocol/vectors.txt")).first { it.exists() }
        return file.readText().split(Regex("\n\\s*\n")).mapNotNull { text ->
            val lines = text.lines().map { it.trim() }.filter { it.isNotEmpty() && !it.startsWith("#") }
            if (lines.isEmpty()) return@mapNotNull null
            val pairs = lines.map { it.substringBefore(':').trim() to it.substringAfter(':').trim() }
            Block(pairs[0].first, pairs[0].second, pairs.drop(1))
        }
    }

    private fun bytes(hex: String) = hex.split(' ').filter { it.isNotEmpty() }.map { it.toInt(16).toByte() }.toByteArray()

    private fun ints(text: String) = text.split(' ').filter { it.isNotEmpty() }.map { it.toInt() }

    @Test
    fun validVectorsDecodeToTheDocumentedValuesAndEncodeBack() {
        val valid = load().filter { it.kind == "vector" }
        assertTrue(valid.size >= 7, "expected at least seven valid vectors")
        for (v in valid) {
            val wire = bytes(v.one("hex"))
            val decoded = PacketCodec.decode(wire)
            assertEquals(v.one("seq").toInt(), decoded.sequence, v.name)
            assertEquals(v.one("first_sample_ms").toLong(), decoded.firstSampleMs, v.name)
            val g = ints(v.one("gravity_mg"))
            assertEquals(Vec3(g[0], g[1], g[2]), decoded.gravityMg, v.name)
            val expected = v.all("sample").map { ints(it) }.map { MotionSample(Vec3(it[0], it[1], it[2]), Vec3(it[3], it[4], it[5])) }
            assertEquals(expected, decoded.samples, v.name)
            val speed = v.fields.firstOrNull { it.first == "speed_cm_s" }?.second
            if (speed == null || speed == "none") {
                assertEquals(null, decoded.speed, v.name)
            } else {
                assertEquals(SpeedFix(speed.toInt(), v.one("fix_age_ms").toInt()), decoded.speed, v.name)
            }
            // Only version 2 is ever sent, so only its vectors encode back to the same bytes.
            if (wire[0].toInt() == 2) assertContentEquals(wire, PacketCodec.encode(decoded), "${v.name}: re-encoding")
        }
    }

    @Test
    fun invalidVectorsAreRejectedWithTheDocumentedError() {
        val invalid = load().filter { it.kind == "invalid" }
        assertTrue(invalid.size >= 9, "expected at least nine invalid vectors")
        for (v in invalid) {
            val error = assertFailsWith<PacketFormatException>(v.name) { PacketCodec.decode(bytes(v.one("hex"))) }.error
            assertEquals(v.one("error").uppercase(), error.name, v.name)
        }
    }
}
