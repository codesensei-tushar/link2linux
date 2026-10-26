package dev.link2linux.android.feature.pairing

import android.content.Context
import android.content.IntentSender
import androidx.lifecycle.ViewModel
import dagger.hilt.android.lifecycle.HiltViewModel
import dagger.hilt.android.qualifiers.ApplicationContext
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import javax.inject.Inject

/** Where the offer of [CompanionOffer] stands. */
internal enum class OfferStep { ASK, WORKING, DONE, NOT_FOUND, UNAVAILABLE, SKIPPED }

/**
 * Holds the companion offer for [CompanionOffer], so it survives a rotation,
 * and takes Android's answers to an association, which can come at any time
 * after the request. The chooser Android asks for is shown by whichever
 * screen is there when it comes, through that screen's own launcher. An
 * answer that comes after the screen is gone, or to an attempt the user has
 * since replaced or skipped, is dropped. Answers arrive on the main thread.
 */
@HiltViewModel
class CompanionOfferViewModel internal constructor(private val setup: CompanionAssociator) : ViewModel() {

    @Inject
    constructor(@ApplicationContext context: Context) : this(CompanionSetup(context))

    private val _step = MutableStateFlow(OfferStep.ASK)
    internal val step: StateFlow<OfferStep> = _step

    private val _chooser = MutableStateFlow<IntentSender?>(null)

    /** A chooser Android asked to show, until the screen has launched it. */
    internal val chooser: StateFlow<IntentSender?> = _chooser

    /** Counts requests; an answer to any but the latest is dropped. */
    private var attempt = 0

    internal val isAvailable: Boolean get() = setup.isAvailable

    internal val permission: String? get() = setup.permission

    internal fun begin(address: String) {
        val current = ++attempt
        _chooser.value = null
        _step.value = OfferStep.WORKING
        setup.associate(
            address,
            object : CompanionSetup.Listener {
                override fun onChooser(sender: IntentSender) {
                    if (current == attempt) _chooser.value = sender
                }

                override fun onAssociated() {
                    if (current == attempt) finish(address)
                }

                override fun onFailure(reason: CompanionSetup.Failure) {
                    if (current != attempt) return
                    _step.value = if (reason == CompanionSetup.Failure.NOT_FOUND) OfferStep.NOT_FOUND else OfferStep.UNAVAILABLE
                }
            },
        )
    }

    internal fun onPermission(granted: Boolean, address: String) {
        if (granted) begin(address) else _step.value = OfferStep.ASK
    }

    internal fun onChooserShown() {
        _chooser.value = null
    }

    internal fun onChooserResult(chosen: Boolean, address: String) {
        if (chosen) finish(address) else _step.value = OfferStep.ASK
    }

    internal fun skip() {
        attempt++
        _chooser.value = null
        _step.value = OfferStep.SKIPPED
    }

    override fun onCleared() {
        // Android may still answer; there is no screen left to show it.
        attempt++
    }

    private fun finish(address: String) {
        setup.startObserving(address)
        _step.value = OfferStep.DONE
    }
}
