package com.amsdots.android

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import com.amsdots.stream.LinkStatus
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/** What the running [MotionService] reports to the screen. Lives as long as the process. */
object ServiceState {
    private val mutable = MutableStateFlow(LinkStatus())
    val status: StateFlow<LinkStatus> = mutable.asStateFlow()

    internal fun update(change: (LinkStatus) -> LinkStatus) = mutable.update(change)
}

/** Runtime permissions the service needs. API 30 has none: the legacy Bluetooth permissions are install-time. */
object RequiredPermissions {
    fun all(): List<String> = buildList {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            add(Manifest.permission.BLUETOOTH_ADVERTISE)
            add(Manifest.permission.BLUETOOTH_CONNECT)
        }
        // Without it the foreground-service notification is hidden, which is the only way to stop the service from outside the app.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) add(Manifest.permission.POST_NOTIFICATIONS)
    }

    fun missing(context: Context): List<String> =
        all().filter { context.checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }

    /** Optional: only for the GPS speed shown on the screen. Asked together with the required ones. */
    const val LOCATION = Manifest.permission.ACCESS_FINE_LOCATION

    fun hasLocation(context: Context): Boolean = context.checkSelfPermission(LOCATION) == PackageManager.PERMISSION_GRANTED
}
