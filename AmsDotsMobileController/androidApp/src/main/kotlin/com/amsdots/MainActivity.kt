package com.amsdots

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import com.amsdots.android.MotionService
import com.amsdots.android.RequiredPermissions
import com.amsdots.android.ServiceState

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)

        setContent {
            val status by ServiceState.status.collectAsState()
            var pendingSensorTest by remember { mutableStateOf(false) }
            val askPermissions = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
                // The service reports what is still missing; notification permission is optional for it to run.
                if (RequiredPermissions.missing(this).none { it != POST_NOTIFICATIONS }) MotionService.start(this, pendingSensorTest)
            }
            App(
                status = status,
                onStart = { sensorTest ->
                    pendingSensorTest = sensorTest
                    val missing = RequiredPermissions.missing(this) +
                        listOfNotNull(RequiredPermissions.LOCATION.takeUnless { RequiredPermissions.hasLocation(this) })
                    if (missing.isEmpty()) MotionService.start(this, sensorTest) else askPermissions.launch(missing.toTypedArray())
                },
                onStop = { MotionService.stop(this) },
            )
        }
    }

    private companion object {
        const val POST_NOTIFICATIONS = "android.permission.POST_NOTIFICATIONS"
    }
}
