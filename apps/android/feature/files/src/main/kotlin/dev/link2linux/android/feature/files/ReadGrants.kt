package dev.link2linux.android.feature.files

import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ClipData
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import java.util.concurrent.atomic.AtomicInteger

/**
 * Keeps the read grants of shared documents for as long as their transfer
 * may need them.
 *
 * A share grants read access to the activity that receives it, and only
 * until that activity is gone, while a transfer may read its files again
 * after a reconnect, up to an hour later. A job carries the documents'
 * grants from the moment it is scheduled until it finishes or is
 * cancelled, so a job that only waits holds them: it is cancelled once the
 * transfer ends, and otherwise ends by itself after [HOLD_MILLIS].
 */
internal object ReadGrants {

    /** Holds read access to [uris] and returns the id that [release] takes, or null when the system refused. */
    fun hold(context: Context, uris: List<Uri>): Int? {
        if (uris.isEmpty()) return null
        val clip = ClipData.newRawUri(null, uris.first())
        uris.drop(1).forEach { clip.addItem(ClipData.Item(it)) }
        val id = JOB_IDS + next.getAndIncrement() % JOB_ID_RANGE
        val job = JobInfo.Builder(id, ComponentName(context, GrantHolderJob::class.java))
            .setClipData(clip, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            .setMinimumLatency(HOLD_MILLIS)
            .build()
        val scheduled = runCatching { context.getSystemService(JobScheduler::class.java).schedule(job) }.getOrNull()
        return id.takeIf { scheduled == JobScheduler.RESULT_SUCCESS }
    }

    fun release(context: Context, id: Int) {
        runCatching { context.getSystemService(JobScheduler::class.java).cancel(id) }
    }

    private val next = AtomicInteger()

    private const val JOB_IDS = 0x4c32_0000
    private const val JOB_ID_RANGE = 1000

    /** Longer than a transfer is kept for a reconnect. */
    private const val HOLD_MILLIS = 2 * 60 * 60 * 1000L
}

/** The job behind [ReadGrants]. It has nothing to do: finishing lets the grants go. */
class GrantHolderJob : JobService() {
    override fun onStartJob(params: JobParameters?): Boolean = false

    override fun onStopJob(params: JobParameters?): Boolean = false
}
