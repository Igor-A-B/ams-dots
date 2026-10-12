package com.amsdots

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeContentPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier

/** Root screen. For now only proves the app builds and runs; the real status screen comes in M5. */
@Composable
fun App() {
    MaterialTheme {
        Column(
            modifier = Modifier.safeContentPadding().fillMaxSize(),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("AmsDots", style = MaterialTheme.typography.headlineMedium)
            Text("Motion sender - M0 hello", style = MaterialTheme.typography.bodyMedium)
        }
    }
}
