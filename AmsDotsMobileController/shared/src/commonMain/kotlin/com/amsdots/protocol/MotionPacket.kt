package com.amsdots.protocol

/** A 3-axis reading with integer components. */
data class Vec3(val x: Int, val y: Int, val z: Int)

/**
 * One moment of motion in the phone's own axes.
 *
 * @property acceleration linear acceleration (gravity removed) in mm/s²
 * @property rotation angular velocity in mrad/s
 */
data class MotionSample(val acceleration: Vec3, val rotation: Vec3) {
    companion object {
        /**
         * Builds a sample from Android sensor values: acceleration in m/s² and
         * angular velocity in rad/s. Values beyond the 16-bit range (about
         * ±32.7 m/s² or rad/s) saturate instead of wrapping around.
         */
        fun fromSensors(acceleration: FloatArray, rotation: FloatArray): MotionSample {
            require(acceleration.size >= 3 && rotation.size >= 3) { "need 3 values per sensor" }
            return MotionSample(
                Vec3(milli(acceleration[0]), milli(acceleration[1]), milli(acceleration[2])),
                Vec3(milli(rotation[0]), milli(rotation[1]), milli(rotation[2])),
            )
        }

        private fun milli(value: Float): Int = (value * 1000f).toInt().coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt())
    }
}

/**
 * One BLE notification.
 *
 * @property sequence counter that wraps at 65536; lets the receiver spot lost or reordered packets
 * @property firstSampleMs time of the first sample in ms since the sensors started (wraps at 2³²)
 * @property gravityMg gravity direction in the phone's axes, in 1/1000 of 9.81 m/s²
 * @property samples 1 to [Protocol.MAX_SAMPLES] samples, [Protocol.SAMPLE_SIZE] bytes each, in time order
 */
data class MotionPacket(
    val sequence: Int,
    val firstSampleMs: Long,
    val gravityMg: Vec3,
    val samples: List<MotionSample>,
)

/** What is wrong with a packet that could not be decoded. */
enum class PacketError { TOO_SHORT, BAD_VERSION, BAD_LENGTH, BAD_COUNT }

class PacketFormatException(val error: PacketError, message: String) : Exception(message)
