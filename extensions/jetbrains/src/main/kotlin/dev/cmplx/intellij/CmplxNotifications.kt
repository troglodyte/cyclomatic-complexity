package dev.cmplx.intellij

import com.intellij.notification.NotificationAction
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.options.ShowSettingsUtil
import com.intellij.openapi.project.Project
import java.util.concurrent.ConcurrentHashMap

internal object CmplxNotifications {

    private const val GROUP = "cmplx"

    /** Projects already warned this session, so opening ten files warns once. */
    private val warned: MutableSet<String> = ConcurrentHashMap.newKeySet()

    fun binaryNotFound(project: Project, configured: String) {
        if (!warned.add(project.locationHash)) return

        val message = if (configured.isBlank()) {
            "cmplx was not found on PATH. Install it with <code>cargo install --path crates/cmplx</code>, " +
                "or set its location in settings."
        } else {
            "cmplx is not executable at <code>$configured</code>."
        }

        NotificationGroupManager.getInstance()
            .getNotificationGroup(GROUP)
            .createNotification("cmplx unavailable", message, NotificationType.WARNING)
            .addAction(
                NotificationAction.createSimple("Configure") {
                    ShowSettingsUtil.getInstance()
                        .showSettingsDialog(project, CmplxConfigurable::class.java)
                }
            )
            .notify(project)
    }
}
