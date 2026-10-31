package com.aibt.sonotheca

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.os.Build
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

/**
 * 把前台服务暴露给前端：`invoke("plugin:fgs|start", { count })`。
 *
 * 由前端驱动是有意为之 —— 任务状态本来就通过 `download://state` 事件实时同步到
 * 前端 store，让 store 直接决定 FGS 的起停，Rust 侧不必再为"当前有几个活跃任务"
 * 维护一份额外状态。
 */
@TauriPlugin
class FgsPlugin(private val activity: Activity) : Plugin(activity) {

    @Command
    fun start(invoke: Invoke) {
        val count = readCount(invoke)
        ensureNotificationPermission()
        DownloadService.start(activity, count)
        invoke.resolve()
    }

    @Command
    fun update(invoke: Invoke) {
        val count = readCount(invoke)
        DownloadService.start(activity, count)
        invoke.resolve()
    }

    @Command
    fun stop(invoke: Invoke) {
        DownloadService.stop(activity)
        invoke.resolve()
    }

    private fun readCount(invoke: Invoke): Int {
        return try {
            val args = invoke.parseArgs(FgsArgs::class.java)
            if (args.count > 0) args.count else 1
        } catch (_: Throwable) {
            1
        }
    }

    /** Android 13+ 通知需要运行时授权；拒绝也不影响服务运行，只是看不到通知 */
    private fun ensureNotificationPermission() {
        if (Build.VERSION.SDK_INT < 33) return
        val granted = activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
        if (!granted) {
            activity.requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 0x5A18)
        }
    }
}

class FgsArgs {
    var count: Int = 1
}
