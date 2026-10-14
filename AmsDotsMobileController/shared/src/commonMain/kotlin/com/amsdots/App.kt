package com.amsdots

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeContentPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import com.amsdots.stream.LinkPhase
import com.amsdots.stream.LinkStatus
import com.amsdots.stream.MotionReadout
import com.amsdots.stream.SpeedReadout
import kotlin.math.PI
import kotlin.math.round

/**
 * Root screen: the minimum to run the MVP, a status line and one start/stop button.
 * The real status screen, permission and battery flows are milestone M5.
 *
 * @param onStart called with `true` when the user wants the sensors-only test instead of the Bluetooth link
 */
@Composable
fun App(status: LinkStatus, onStart: (sensorTest: Boolean) -> Unit, onStop: () -> Unit) {
    var sensorTest by rememberSaveable { mutableStateOf(false) }
    MaterialTheme {
        Column(
            // Scrolls so the button stays reachable in landscape, where the debug figures fill the screen.
            modifier = Modifier.safeContentPadding().fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("AmsDots", style = MaterialTheme.typography.headlineMedium)
            Text(phaseText(status.phase), style = MaterialTheme.typography.titleMedium)
            status.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            status.measured?.let {
                Text("${oneDecimal(it.rateHz)} Hz (target ${status.targetRateHz}), jitter ${oneDecimal(it.jitterMs)} ms")
            }
            if (status.phase == LinkPhase.STREAMING) Text("${status.packetsSent} packets sent")
            if (status.running) Debug(status.readout, status.speed)

            if (status.running) {
                Button(onClick = onStop) { Text("Stop") }
            } else {
                Button(onClick = { onStart(sensorTest) }) { Text("Start") }
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Switch(checked = sensorTest, onCheckedChange = { sensorTest = it })
                    Text("Sensor test (no Bluetooth)")
                }
            }
        }
    }
}

/** Live figures for checking the sensors in the car; the same split by gravity that the laptop uses. */
@Composable
private fun Debug(readout: MotionReadout?, speed: SpeedReadout?) {
    val lines = buildList {
        add(
            if (speed == null) "Speed       - (no GPS fix)"
            else "Speed       ${fixed(speed.metersPerSecond * 3.6, 0)} km/h" + (speed.accuracyMeters?.let { " (±${fixed(it, 0)} m)" } ?: ""),
        )
        add("GPS accel   " + (speed?.acceleration?.let { "${signed(it)} m/s²" } ?: "-"))
        if (readout == null) {
            add("Sensors     not running")
        } else {
            add("Accel horiz ${fixed(readout.horizontal, 2)} m/s² (peak ${fixed(readout.horizontalPeak, 2)})")
            add("Accel vert  ${signed(readout.vertical)} m/s²")
            add("Turning     ${signed(readout.yaw)} rad/s (${signed(readout.yaw * 180 / PI, 0)}°/s)")
            add("Rotation    peak ${fixed(readout.rotationPeak, 2)} rad/s")
            add("Tilt        ${fixed(readout.tiltDegrees, 0)}° (0 flat, 90 upright)")
            add("Samples     ${readout.samples} per ${READOUT_WINDOW}")
        }
    }
    Column(horizontalAlignment = Alignment.Start) {
        for (line in lines) Text(line, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodyMedium)
    }
}

private const val READOUT_WINDOW = "0.25 s"

private fun fixed(value: Double, decimals: Int): String {
    var factor = 1.0
    repeat(decimals) { factor *= 10 }
    val rounded = round(value * factor) / factor
    if (decimals == 0) return rounded.toLong().toString()
    val text = rounded.toString()
    val dot = text.indexOf('.')
    return if (dot < 0) text + "." + "0".repeat(decimals) else text + "0".repeat(maxOf(0, decimals - (text.length - dot - 1)))
}

private fun signed(value: Double, decimals: Int = 2): String = (if (value >= 0) "+" else "") + fixed(value, decimals)

private fun phaseText(phase: LinkPhase): String = when (phase) {
    LinkPhase.STOPPED -> "Stopped"
    LinkPhase.ADVERTISING -> "Waiting for the laptop"
    LinkPhase.CONNECTED -> "Laptop connected, not streaming yet"
    LinkPhase.STREAMING -> "Streaming"
    LinkPhase.SENSOR_TEST -> "Sensor test"
    LinkPhase.ERROR -> "Stopped"
}

private fun oneDecimal(value: Double): String = (round(value * 10) / 10).toString()
