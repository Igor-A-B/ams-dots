package com.amsdots.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.bluetooth.BluetoothAdapter
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import com.amsdots.stream.LinkPhase
import com.amsdots.stream.LinkStatus
import com.amsdots.stream.MotionStreamer
import com.amsdots.stream.RateMeter
import com.amsdots.stream.SensorFuser

/**
 * Foreground service (type `connectedDevice`) that owns the whole phone side:
 * the GATT peripheral, the sensors and the packet pipeline.
 *
 * Sensors run only while the laptop is subscribed; while it is not, the only
 * cost is the low-power advertising. Everything runs on one background thread
 * so the pipeline needs no locks.
 *
 * In *sensor test* mode there is no Bluetooth: the sensors run on their own and
 * the achieved rate is logged (tag `AmsDots`) and shown, to measure the sensors'
 * delivery and energy cost separately from the radio.
 */
class MotionService : Service(), PeripheralEvents {
    private lateinit var thread: HandlerThread
    private lateinit var handler: Handler
    private lateinit var fuser: SensorFuser
    private lateinit var sensors: MotionSensors
    private lateinit var streamer: MotionStreamer
    private lateinit var peripheral: GattPeripheral
    private val meter = RateMeter()

    private var wakeLock: PowerManager.WakeLock? = null
    private var started = false
    private var sensorsOn = false
    private var sensorTest = false
    private var mtu = DEFAULT_MTU
    private var rateHz = LinkStatus.DEFAULT_RATE_HZ

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        thread = HandlerThread("AmsDots-pipeline").also { it.start() }
        handler = Handler(thread.looper)

        peripheral = GattPeripheral(this, post = { handler.post(it) }, events = this)
        streamer = MotionStreamer(peripheral)
        peripheral.notificationSentListener = { streamer.onNotificationSent() }
        fuser = SensorFuser { sample ->
            meter.add(sample.timeNs)
            if (!sensorTest) streamer.onSample(sample)
        }
        sensors = MotionSensors(this, handler, fuser)

        val filter = IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) registerReceiver(bluetoothState, filter, RECEIVER_NOT_EXPORTED)
        else registerReceiver(bluetoothState, filter)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            shutdown(null)
            return START_NOT_STICKY
        }
        // Must happen within seconds of startForegroundService().
        enterForeground(LinkPhase.STOPPED)
        if (!started) {
            started = true
            sensorTest = intent?.getBooleanExtra(EXTRA_SENSOR_TEST, false) ?: false
            handler.post { begin() }
        }
        // Not sticky: restarting after a process kill is the recovery work of milestone M5.
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        unregisterReceiver(bluetoothState)
        handler.post { teardown() }
        thread.quitSafely()
        ServiceState.update { it.copy(phase = if (it.error != null) LinkPhase.ERROR else LinkPhase.STOPPED, measured = null, packetsSent = 0) }
        super.onDestroy()
    }

    // --- lifecycle on the pipeline thread ---

    private fun begin() {
        sensors.missing?.let { return shutdown("This phone has no $it") }
        Log.i(TAG, "sensors: ${sensors.describe()}")
        val missingPermissions = RequiredPermissions.missing(this)
        if (missingPermissions.isNotEmpty() && !sensorTest) return shutdown("Missing permission: ${missingPermissions.first().substringAfterLast('.')}")

        if (sensorTest) {
            setPhase(LinkPhase.SENSOR_TEST)
            startSensors()
        } else {
            peripheral.start()?.let { return shutdown(it) }
            setPhase(LinkPhase.ADVERTISING) // refined by onAdvertising()
        }
    }

    private fun teardown() {
        stopSensors()
        peripheral.stop()
        streamer.reset()
    }

    private fun shutdown(error: String?) {
        if (error != null) Log.w(TAG, error)
        handler.post {
            teardown()
            ServiceState.update { it.copy(phase = if (error == null) LinkPhase.STOPPED else LinkPhase.ERROR, error = error, measured = null) }
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
    }

    // --- PeripheralEvents ---

    override fun onAdvertising() = setPhase(LinkPhase.ADVERTISING)

    override fun onConnected(connected: Boolean) {
        if (connected) {
            mtu = DEFAULT_MTU
            setPhase(LinkPhase.CONNECTED)
        } else {
            stopSensors()
            streamer.reset()
            setPhase(LinkPhase.ADVERTISING)
        }
    }

    override fun onMtu(mtu: Int) {
        this.mtu = mtu
        streamer.onMtu(mtu)
    }

    override fun onSubscribed(subscribed: Boolean) {
        if (subscribed) {
            streamer.reset()
            streamer.onMtu(mtu)
            startSensors()
            setPhase(LinkPhase.STREAMING)
        } else {
            stopSensors()
            streamer.reset()
            setPhase(LinkPhase.CONNECTED)
        }
    }

    override fun onSetRate(hz: Int) {
        Log.i(TAG, "laptop asked for $hz Hz")
        rateHz = hz
        ServiceState.update { it.copy(targetRateHz = hz) }
        if (sensorsOn) startSensors()
    }

    override fun onError(message: String) = shutdown(message)

    // --- sensors ---

    private fun startSensors() {
        fuser.reset()
        meter.reset()
        sensors.start(rateHz)
        sensorsOn = true
        // Plain (non-wake-up) sensors stop delivering when the CPU suspends, so hold it awake instead.
        if (!sensors.allWakeUp && wakeLock == null) {
            Log.w(TAG, "not all sensors are wake-up sensors: holding a partial wake lock while streaming")
            wakeLock = getSystemService(PowerManager::class.java)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "AmsDots:streaming")
                .also { it.acquire(MAX_WAKE_LOCK_MS) }
        }
        handler.removeCallbacks(reportRate)
        handler.postDelayed(reportRate, REPORT_INTERVAL_MS)
    }

    private fun stopSensors() {
        sensors.stop()
        sensorsOn = false
        handler.removeCallbacks(reportRate)
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
    }

    /** Every few seconds: log the achieved rate and jitter and show them on the screen. */
    private val reportRate = object : Runnable {
        override fun run() {
            val report = meter.report()
            meter.reset()
            if (report != null) {
                Log.i(TAG, "rate %.1f Hz (target %d), jitter %.2f ms, max gap %.1f ms, %d samples, packets %d, dropped %d".format(
                    report.rateHz, rateHz, report.jitterMs, report.maxGapMs, report.samples, streamer.packetsSent, streamer.samplesDropped))
            }
            ServiceState.update { it.copy(measured = report, packetsSent = streamer.packetsSent) }
            if (sensorsOn) handler.postDelayed(this, REPORT_INTERVAL_MS)
        }
    }

    // --- status and notification ---

    private fun setPhase(phase: LinkPhase) {
        ServiceState.update { it.copy(phase = phase, error = null, measured = if (phase == LinkPhase.STREAMING || phase == LinkPhase.SENSOR_TEST) it.measured else null, packetsSent = if (phase == LinkPhase.STREAMING) it.packetsSent else 0) }
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(phase))
    }

    private fun enterForeground(phase: LinkPhase) {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL_ID, "AmsDots link", NotificationManager.IMPORTANCE_LOW))
        startForeground(NOTIFICATION_ID, notification(phase), ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
    }

    private fun notification(phase: LinkPhase): Notification {
        val text = when (phase) {
            LinkPhase.STOPPED -> "Starting"
            LinkPhase.ADVERTISING -> "Waiting for the laptop"
            LinkPhase.CONNECTED -> "Laptop connected"
            LinkPhase.STREAMING -> "Streaming motion to the laptop"
            LinkPhase.SENSOR_TEST -> "Sensor test (no Bluetooth)"
            LinkPhase.ERROR -> "Stopped"
        }
        val open = packageManager.getLaunchIntentForPackage(packageName)?.let {
            PendingIntent.getActivity(this, 0, it, PendingIntent.FLAG_IMMUTABLE)
        }
        val stop = PendingIntent.getService(this, 1, Intent(this, MotionService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.stat_sys_data_bluetooth)
            .setContentTitle("AmsDots")
            .setContentText(text)
            .setOngoing(true)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(null, "Stop", stop).build())
            .build()
    }

    /** Bluetooth switched off while running: the link cannot work, so stop and say why. */
    private val bluetoothState = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (sensorTest) return
            if (intent.getIntExtra(BluetoothAdapter.EXTRA_STATE, -1) == BluetoothAdapter.STATE_OFF) shutdown("Bluetooth was turned off")
        }
    }

    companion object {
        private const val TAG = "AmsDots"
        private const val ACTION_STOP = "com.amsdots.STOP"
        private const val EXTRA_SENSOR_TEST = "sensor_test"
        private const val CHANNEL_ID = "link"
        private const val NOTIFICATION_ID = 1
        private const val DEFAULT_MTU = 23
        private const val REPORT_INTERVAL_MS = 5_000L
        private const val MAX_WAKE_LOCK_MS = 6 * 60 * 60 * 1000L

        /** Starts the service; the caller must already hold the runtime permissions ([RequiredPermissions]). */
        fun start(context: Context, sensorTest: Boolean = false) {
            context.startForegroundService(Intent(context, MotionService::class.java).putExtra(EXTRA_SENSOR_TEST, sensorTest))
        }

        fun stop(context: Context) {
            context.startService(Intent(context, MotionService::class.java).setAction(ACTION_STOP))
        }
    }
}
