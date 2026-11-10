package app.homespace.android

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.pm.ServiceInfo
import android.os.Build
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundInfo
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.homespace_core.CoreException
import uniffi.homespace_core.SyncSettings

private const val PERIODIC = "auto-upload-periodic"
private const val ON_NEW_MEDIA = "auto-upload-new-media"
private const val TAG_NEW_MEDIA = "new-media"
private const val PERIOD_MINUTES = 15L
private const val CHANNEL = "auto-upload"
private const val NOTIFICATION = 1
private const val SETTLE_PASSES = 3
private const val SETTLE_MILLIS = 6_000L

/** Runs passes while the app isn't open, when "Sync in background" is on. */
class SyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val session = try {
            ClientHolder.resume(applicationContext)
        } catch (e: CoreException) {
            return if (e is CoreException.Unauthorized) Result.success() else Result.retry()
        } ?: return Result.success()
        val auto = session.autoUpload
        val settings = auto.settings()
        if (!settings.background) return Result.success()
        // After a new photo: it is one shot, so ask to be woken by the next as well. Appended,
        // because replacing the unique work would cancel this very worker.
        if (TAG_NEW_MEDIA in tags) {
            scheduleOnNewMedia(applicationContext, settings, ExistingWorkPolicy.APPEND_OR_REPLACE)
        }
        return try {
            coroutineScope {
                val notify = launch { showOnceThereIsWork(session) }
                try {
                    // A new photo wakes this at once, and a pass leaves a file that changed just
                    // now for later. Without another pass, "later" would be the next period.
                    for (attempt in 1..SETTLE_PASSES) {
                        val summary = auto.runPass(false)
                        if (summary.pending == 0uL || summary.outage || summary.stopped) break
                        delay(SETTLE_MILLIS)
                    }
                    session.client.offlinePass()
                    // No socket is open in the background, so this is how a transfer gets noticed.
                    runCatching { notifyIncoming(applicationContext, session.client.transfers().received) }
                    // Nothing watches the network once this job ends. An open app allows it again.
                    session.client.offlineAllow(false)
                } finally {
                    notify.cancel()
                }
            }
            Result.success()
        } catch (e: CancellationException) {
            // Android took back the constraint. A pass runs on regardless of this coroutine.
            auto.stop()
            withContext(NonCancellable) { runCatching { session.client.offlineAllow(false) } }
            throw e
        } catch (_: CoreException) {
            Result.success() // The next run tries again, and the settings screen shows the problem.
        }
    }

    /** A notification only when there are files to send, not every 15 minutes. */
    private suspend fun showOnceThereIsWork(session: ActiveSession) {
        ClientHolder.syncChanges.first {
            session.autoUpload.sources().any { source -> source.pending > 0u } ||
                session.client.offlineStatus().pending > 0u
        }
        try {
            setForeground(getForegroundInfo())
        } catch (_: IllegalStateException) {
            // Android doesn't allow it right now. The upload still runs within the job's limits.
        }
    }

    override suspend fun getForegroundInfo(): ForegroundInfo {
        val manager = applicationContext.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Sync", NotificationManager.IMPORTANCE_LOW),
        )
        val notification = android.app.Notification.Builder(applicationContext, CHANNEL)
            .setContentTitle("Syncing with Home Space")
            .setSmallIcon(R.drawable.ic_settings)
            .setOngoing(true)
            .build()
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ForegroundInfo(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            ForegroundInfo(NOTIFICATION, notification)
        }
    }
}

private fun networkConstraint(settings: SyncSettings): NetworkType =
    // WorkManager has no "Wi-Fi": unmetered is as close as it gets.
    if (settings.wifiOnly) NetworkType.UNMETERED else NetworkType.CONNECTED

private fun scheduleOnNewMedia(
    context: Context,
    settings: SyncSettings,
    policy: ExistingWorkPolicy = ExistingWorkPolicy.REPLACE,
) {
    val constraints = Constraints.Builder()
        .setRequiredNetworkType(networkConstraint(settings))
        .addContentUriTrigger(imagesUri(), true)
        .addContentUriTrigger(videosUri(), true)
        .build()
    val request = OneTimeWorkRequestBuilder<SyncWorker>()
        .setConstraints(constraints)
        .addTag(TAG_NEW_MEDIA)
        .build()
    WorkManager.getInstance(context)
        .enqueueUniqueWork(ON_NEW_MEDIA, policy, request)
}

/** Brings the background jobs in line with the settings. */
fun applyBackgroundSetting(context: Context, settings: SyncSettings) {
    if (!settings.background) {
        cancelBackgroundSync(context)
        return
    }
    val constraints = Constraints.Builder()
        .setRequiredNetworkType(networkConstraint(settings))
        .build()
    val periodic = PeriodicWorkRequestBuilder<SyncWorker>(PERIOD_MINUTES, TimeUnit.MINUTES)
        .setConstraints(constraints)
        .build()
    WorkManager.getInstance(context)
        .enqueueUniquePeriodicWork(PERIODIC, ExistingPeriodicWorkPolicy.UPDATE, periodic)
    scheduleOnNewMedia(context, settings)
}

fun cancelBackgroundSync(context: Context) {
    val manager = WorkManager.getInstance(context)
    manager.cancelUniqueWork(PERIODIC)
    manager.cancelUniqueWork(ON_NEW_MEDIA)
}
