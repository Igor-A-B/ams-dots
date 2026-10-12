package com.amsdots.protocol

/** Commands the laptop writes to the control characteristic. */
sealed interface ControlCommand {
    /** Ask the phone to sample at [hz] (within [Protocol.MIN_RATE_HZ]..[Protocol.MAX_RATE_HZ]). */
    data class SetRate(val hz: Int) : ControlCommand {
        init {
            require(hz in Protocol.MIN_RATE_HZ..Protocol.MAX_RATE_HZ) { "rate must be ${Protocol.MIN_RATE_HZ}..${Protocol.MAX_RATE_HZ} Hz" }
        }
    }

    companion object {
        private const val SET_RATE: Byte = 0x01

        fun encode(command: ControlCommand): ByteArray = when (command) {
            is SetRate -> byteArrayOf(SET_RATE, command.hz.toByte())
        }

        /** Decodes a write, or returns `null` for anything unknown or malformed (the phone ignores those). */
        fun decode(bytes: ByteArray): ControlCommand? {
            if (bytes.size == 2 && bytes[0] == SET_RATE) {
                val hz = bytes[1].toInt() and 0xFF
                if (hz in Protocol.MIN_RATE_HZ..Protocol.MAX_RATE_HZ) return SetRate(hz)
            }
            return null
        }
    }
}
