package com.amsdots.android

import android.annotation.SuppressLint
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothStatusCodes
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.ParcelUuid
import android.util.Log
import com.amsdots.protocol.ControlCommand
import com.amsdots.protocol.Protocol
import com.amsdots.stream.NotificationSink
import java.util.UUID

/** What the GATT peripheral tells the service. Every call arrives on the service's handler thread. */
interface PeripheralEvents {
    fun onAdvertising()
    fun onConnected(connected: Boolean)
    fun onMtu(mtu: Int)
    fun onSubscribed(subscribed: Boolean)
    fun onSetRate(hz: Int)
    fun onError(message: String)
}

/**
 * The phone's side of the link: a GATT server with the AmsDots service and an
 * advertiser. One central (the laptop) at a time. Motion goes out as
 * notifications of the motion characteristic; the control characteristic
 * accepts [ControlCommand]s. The wire format is in `docs/protocol.md`.
 *
 * The advertiser runs in balanced mode (four a second): the only cost while nobody is
 * connected. BLE connection parameters are chosen by the central, so the phone
 * cannot (and need not) request a connection priority.
 */
@SuppressLint("MissingPermission") // the service checks the Bluetooth permissions before it starts this
class GattPeripheral(
    private val context: Context,
    private val handler: Handler,
    private val events: PeripheralEvents,
) : NotificationSink {
    private val manager = context.getSystemService(BluetoothManager::class.java)
    private var server: BluetoothGattServer? = null
    private var advertiser: BluetoothLeAdvertiser? = null
    private var motion: BluetoothGattCharacteristic? = null

    @Volatile private var central: BluetoothDevice? = null
    @Volatile private var subscribed = false
    private var advertising = false

    private fun post(block: () -> Unit) {
        handler.post(block)
    }

    /** Opens the server; advertising starts once the service is registered. Returns an error message, or `null`. */
    fun start(): String? {
        val adapter = manager?.adapter ?: return "This phone has no Bluetooth"
        if (!adapter.isEnabled) return "Bluetooth is off"
        advertiser = adapter.bluetoothLeAdvertiser ?: return "This phone cannot advertise over Bluetooth LE"
        val gatt = manager.openGattServer(context, serverCallback) ?: return "Could not open the Bluetooth GATT server"
        server = gatt

        val service = BluetoothGattService(UUID.fromString(Protocol.SERVICE_UUID), BluetoothGattService.SERVICE_TYPE_PRIMARY)
        val motionChar = BluetoothGattCharacteristic(UUID.fromString(Protocol.MOTION_UUID), BluetoothGattCharacteristic.PROPERTY_NOTIFY, 0)
        motionChar.addDescriptor(BluetoothGattDescriptor(CCCD, BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE))
        val controlChar = BluetoothGattCharacteristic(
            UUID.fromString(Protocol.CONTROL_UUID),
            BluetoothGattCharacteristic.PROPERTY_WRITE or BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE,
            BluetoothGattCharacteristic.PERMISSION_WRITE,
        )
        service.addCharacteristic(motionChar)
        service.addCharacteristic(controlChar)
        motion = motionChar
        if (!gatt.addService(service)) return "Could not register the AmsDots GATT service"
        return null
    }

    fun stop() {
        stopAdvertising()
        central = null
        subscribed = false
        server?.close()
        server = null
        motion = null
    }

    /** Set by the service: called when the stack is ready for the next notification. */
    var notificationSentListener: () -> Unit = {}

    /** Starts one notification; the next may start once [notificationSentListener] has been called. */
    override fun send(bytes: ByteArray): Boolean {
        val gatt = server ?: return false
        val device = central ?: return false
        val characteristic = motion ?: return false
        if (!subscribed) return false
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            gatt.notifyCharacteristicChanged(device, characteristic, false, bytes) == BluetoothStatusCodes.SUCCESS
        } else {
            @Suppress("DEPRECATION")
            characteristic.value = bytes
            @Suppress("DEPRECATION")
            gatt.notifyCharacteristicChanged(device, characteristic, false)
        }
    }

    private fun startAdvertising() {
        if (advertising) return
        val settings = AdvertiseSettings.Builder()
            // Four advertisements a second: the laptop notices the phone within a moment, at a cost that only
            // matters while waiting. The mode is never changed while advertising, because restarting the
            // advertiser gives it a new random address and a laptop that is just connecting would lose it.
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_BALANCED)
            .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_HIGH)
            .setConnectable(true)
            .setTimeout(0)
            .build()
        // A 128-bit UUID plus flags nearly fills the 31-byte packet, so the phone's Bluetooth name goes into the
        // scan response (a second packet the laptop asks for). The laptop finds phones by the service UUID and
        // lists them by name, since the random address changes every time the advertiser starts.
        val data = AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .addServiceUuid(ParcelUuid(UUID.fromString(Protocol.SERVICE_UUID)))
            .build()
        val scanResponse = AdvertiseData.Builder().setIncludeDeviceName(true).build()
        advertiser?.startAdvertising(settings, data, scanResponse, advertiseCallback)
    }

    private fun stopAdvertising() {
        if (advertising) advertiser?.stopAdvertising(advertiseCallback)
        advertising = false
    }

    private val advertiseCallback = object : AdvertiseCallback() {
        override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
            advertising = true
            post { events.onAdvertising() }
        }

        override fun onStartFailure(errorCode: Int) {
            advertising = false
            post { events.onError("Advertising failed (code $errorCode)") }
        }
    }

    private val serverCallback = object : BluetoothGattServerCallback() {
        override fun onServiceAdded(status: Int, service: BluetoothGattService) {
            if (status == BluetoothGatt.GATT_SUCCESS) startAdvertising()
            else post { events.onError("Registering the GATT service failed (status $status)") }
        }

        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            Log.i(TAG, "connection ${device.address} status=$status state=$newState")
            if (newState == BluetoothProfile.STATE_CONNECTED) {
                central = device
                // Only one central; the advertiser stops being useful until it leaves.
                stopAdvertising()
                post { events.onConnected(true) }
            } else if (device == central || central == null) {
                central = null
                val wasSubscribed = subscribed
                subscribed = false
                post {
                    if (wasSubscribed) events.onSubscribed(false)
                    events.onConnected(false)
                    startAdvertising()
                }
            }
        }

        override fun onMtuChanged(device: BluetoothDevice, mtu: Int) {
            Log.i(TAG, "mtu $mtu")
            post { events.onMtu(mtu) }
        }

        override fun onNotificationSent(device: BluetoothDevice, status: Int) {
            if (status != BluetoothGatt.GATT_SUCCESS) Log.w(TAG, "notification failed, status $status")
            post { notificationSentListener() }
        }

        override fun onDescriptorWriteRequest(
            device: BluetoothDevice, requestId: Int, descriptor: BluetoothGattDescriptor,
            preparedWrite: Boolean, responseNeeded: Boolean, offset: Int, value: ByteArray,
        ) {
            if (descriptor.uuid != CCCD || descriptor.characteristic.uuid != UUID.fromString(Protocol.MOTION_UUID) || value.size != 2) {
                if (responseNeeded) server?.sendResponse(device, requestId, BluetoothGatt.GATT_REQUEST_NOT_SUPPORTED, offset, null)
                return
            }
            val enable = value[0].toInt() == 1 // 0x0001 = notifications; indications (0x0002) are not offered
            subscribed = enable
            if (responseNeeded) server?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, value)
            Log.i(TAG, "subscribed=$enable")
            post { events.onSubscribed(enable) }
        }

        override fun onDescriptorReadRequest(device: BluetoothDevice, requestId: Int, offset: Int, descriptor: BluetoothGattDescriptor) {
            val value = if (subscribed) BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE else BluetoothGattDescriptor.DISABLE_NOTIFICATION_VALUE
            server?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, value)
        }

        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice, requestId: Int, characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean, responseNeeded: Boolean, offset: Int, value: ByteArray,
        ) {
            val isControl = characteristic.uuid == UUID.fromString(Protocol.CONTROL_UUID)
            if (isControl && !preparedWrite) {
                // Unknown or malformed commands are ignored, but still acknowledged.
                val command = ControlCommand.decode(value)
                if (command is ControlCommand.SetRate) post { events.onSetRate(command.hz) }
            }
            if (responseNeeded) {
                val status = if (isControl) BluetoothGatt.GATT_SUCCESS else BluetoothGatt.GATT_WRITE_NOT_PERMITTED
                server?.sendResponse(device, requestId, status, offset, null)
            }
        }

        override fun onCharacteristicReadRequest(device: BluetoothDevice, requestId: Int, offset: Int, characteristic: BluetoothGattCharacteristic) {
            server?.sendResponse(device, requestId, BluetoothGatt.GATT_READ_NOT_PERMITTED, offset, null)
        }

        override fun onExecuteWrite(device: BluetoothDevice, requestId: Int, execute: Boolean) {
            server?.sendResponse(device, requestId, BluetoothGatt.GATT_REQUEST_NOT_SUPPORTED, 0, null)
        }
    }

    private companion object {
        const val TAG = "AmsDots"

        /** Client Characteristic Configuration descriptor, the standard switch for notifications. */
        val CCCD: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
    }
}
