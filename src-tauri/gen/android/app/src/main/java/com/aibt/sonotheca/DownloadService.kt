package com.aibt.sonotheca

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat

/**
 * 下载前台服务。
 *
 * 息屏后 Android 会冻结后台进程并断开它的网络，只有前台服务（FGS）能维持。
 * 通知栏只保留一条聚合通知（setGroup + setGroupSummary），不按任务刷屏。
 */
class DownloadService : Service() {

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopForegroundCompat()
                stopSelf()
                return START_NOT_STICKY
            }
            else -> {
                val count = intent?.getIntExtra(EXTRA_COUNT, 1) ?: 1
                ensureChannel()
                startForegroundCompat(buildNotification(count))
            }
        }
        return START_STICKY
    }

    private fun startForegroundCompat(notification: Notification) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun stopForegroundCompat() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
    }

    private fun ensureChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (manager.getNotificationChannel(CHANNEL_ID) != null) return

        val channel = NotificationChannel(
            CHANNEL_ID,
            "下载任务",
            NotificationManager.IMPORTANCE_LOW
        ).apply {
            description = "Sonotheca 后台下载进度"
            setShowBadge(false)
        }
        manager.createNotificationChannel(channel)
    }

    private fun buildNotification(count: Int): Notification {
        val openApp = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            },
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

        val summary = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Sonotheca")
            .setContentText(if (count > 1) "正在下载 $count 个任务" else "正在下载 1 个任务")
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setShowWhen(false)
            .setContentIntent(openApp)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .setGroup(GROUP_KEY)
            .setGroupSummary(true)
            .build()

        return summary
    }

    companion object {
        const val CHANNEL_ID = "sonotheca.downloads"
        const val GROUP_KEY = "sonotheca.downloads.group"
        const val NOTIFICATION_ID = 0x5A17
        const val EXTRA_COUNT = "count"
        const val ACTION_STOP = "com.aibt.sonotheca.DOWNLOAD_STOP"

        fun start(context: Context, count: Int) {
            val intent = Intent(context, DownloadService::class.java).apply {
                putExtra(EXTRA_COUNT, count)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
        }

        fun stop(context: Context) {
            val intent = Intent(context, DownloadService::class.java).apply {
                action = ACTION_STOP
            }
            context.startService(intent)
        }
    }
}
