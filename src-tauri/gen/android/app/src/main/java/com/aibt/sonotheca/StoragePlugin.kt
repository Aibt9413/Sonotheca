package com.aibt.sonotheca

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.Settings
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/**
 * 全局存储权限（MANAGE_EXTERNAL_STORAGE）。
 *
 * Android 11+ 这个权限不能弹运行时对话框，只能跳系统设置让用户手动开
 * "所有文件访问权限"。所以这里只有两个能力：查询状态、跳设置页。
 */
@TauriPlugin
class StoragePlugin(private val activity: Activity) : Plugin(activity) {

    @Command
    fun status(invoke: Invoke) {
        val result = JSObject()
        result.put("granted", hasAllFilesAccess())
        invoke.resolve(result)
    }

    @Command
    fun request(invoke: Invoke) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            // 优先直达本应用的授权页；个别 ROM 没有这个页面则退回总列表
            val direct = Intent(
                Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                Uri.parse("package:${activity.packageName}")
            )
            try {
                activity.startActivity(direct)
            } catch (_: Exception) {
                try {
                    activity.startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
                } catch (_: Exception) {
                    // 再退一步：应用详情页，用户自己找权限
                    activity.startActivity(
                        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${activity.packageName}"))
                    )
                }
            }
        } else {
            // Android 10 及以下用传统存储权限
            activity.requestPermissions(
                arrayOf(Manifest.permission.WRITE_EXTERNAL_STORAGE, Manifest.permission.READ_EXTERNAL_STORAGE),
                0x5A19
            )
        }
        invoke.resolve()
    }

    private fun hasAllFilesAccess(): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            Environment.isExternalStorageManager()
        } else {
            ContextCompat.checkSelfPermission(
                activity,
                Manifest.permission.WRITE_EXTERNAL_STORAGE
            ) == PackageManager.PERMISSION_GRANTED
        }
    }
}
