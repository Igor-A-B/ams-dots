package com.amsdots

import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.ui.unit.dp
import com.amsdots.stream.LinkPhase
import com.amsdots.stream.LinkStatus
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
            modifier = Modifier.safeContentPadding().fillMaxSize().padding(16.dp),
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

private fun phaseText(phase: LinkPhase): String = when (phase) {
    LinkPhase.STOPPED -> "Stopped"
    LinkPhase.ADVERTISING -> "Waiting for the laptop"
    LinkPhase.CONNECTED -> "Laptop connected, not streaming yet"
    LinkPhase.STREAMING -> "Streaming"
    LinkPhase.SENSOR_TEST -> "Sensor test"
    LinkPhase.ERROR -> "Stopped"
}

private fun oneDecimal(value: Double): String = (round(value * 10) / 10).toString()
