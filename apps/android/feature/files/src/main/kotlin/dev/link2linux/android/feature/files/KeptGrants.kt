package dev.link2linux.android.feature.files

/**
 * Read grants the app keeps for documents it sends, counted per document:
 * one picked for two sends keeps its grant until both have ended.
 */
internal class KeptGrants<K>(
    /** Keeps the grant of a document; false when that is not possible. */
    private val take: (K) -> Boolean,
    private val release: (K) -> Unit,
) {
    private val counts = HashMap<K, Int>() // guarded by this

    /** Keeps [document]'s grant for one more send; false when it could not be kept. */
    @Synchronized
    fun keep(document: K): Boolean {
        val count = counts[document] ?: 0
        if (count == 0 && !take(document)) return false
        counts[document] = count + 1
        return true
    }

    /** One send that kept [document] ended; the last one lets its grant go. */
    @Synchronized
    fun letGo(document: K) {
        val count = counts[document] ?: return
        if (count > 1) {
            counts[document] = count - 1
        } else {
            counts.remove(document)
            release(document)
        }
    }

    /** Lets go of grants kept by an earlier run, whose transfers ended with it. */
    @Synchronized
    fun letGoOfStale(held: List<K>) {
        held.filter { it !in counts }.forEach(release)
    }
}
