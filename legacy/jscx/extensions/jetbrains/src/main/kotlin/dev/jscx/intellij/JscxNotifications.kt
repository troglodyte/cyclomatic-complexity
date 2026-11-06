package dev.jscx.intellij

import com.intellij.notification.NotificationAction
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.options.ShowSettingsUtil
import com.intellij.openapi.project.Project
import java.util.concurrent.ConcurrentHashMap

internal object JscxNotifications {

    private const val GROUP = "jscx"

    /** Projects already warned this session, so opening ten files warns once. */
    private val warned: MutableSet<String> = ConcurrentHashMap.newKeySet()

    fun binaryNotFound(project: Project, configured: String) {
        if (!warned.add(project.locationHash)) return

        val message = if (configured.isBlank()) {
            "jscx was not found on PATH. Install it with <code>cargo install --path .</code>, " +
                "or set its location in settings."
        } else {
            "jscx is not executable at <code>$configured</code>."
        }

        NotificationGroupManager.getInstance()
            .getNotificationGroup(GROUP)
            .createNotification("jscx unavailable", message, NotificationType.WARNING)
            .addAction(
                NotificationAction.createSimple("Configure") {
                    ShowSettingsUtil.getInstance()
                        .showSettingsDialog(project, JscxConfigurable::class.java)
                }
            )
            .notify(project)
    }
}
