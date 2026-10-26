package dev.link2linux.android.feature.pairing

import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import org.junit.Assert.assertEquals
import org.junit.Test

class CompanionOfferViewModelTest {
    private val address = "0C:1A:2B:3C:4D:5E"

    private class FakeSetup : CompanionAssociator {
        override val isAvailable = true
        override val permission: String? = null
        val requests = mutableListOf<CompanionSetup.Listener>()
        val observing = mutableListOf<String>()

        override fun associate(address: String, listener: CompanionSetup.Listener) {
            requests += listener
        }

        override fun startObserving(address: String) {
            observing += address
        }
    }

    private val setup = FakeSetup()
    private val store = ViewModelStore()
    private val model = ViewModelProvider(store, viewModelFactory { initializer { CompanionOfferViewModel(setup) } })[
        CompanionOfferViewModel::class.java,
    ]

    @Test
    fun `android's answer to the latest request finishes the offer`() {
        model.begin(address)
        assertEquals(OfferStep.WORKING, model.step.value)
        setup.requests.single().onAssociated()
        assertEquals(OfferStep.DONE, model.step.value)
        assertEquals(listOf(address), setup.observing)
    }

    @Test
    fun `an answer that comes after the screen is gone changes nothing`() {
        model.begin(address)
        store.clear()
        setup.requests.single().onAssociated()
        setup.requests.single().onFailure(CompanionSetup.Failure.NOT_FOUND)
        assertEquals(emptyList<String>(), setup.observing)
        assertEquals(OfferStep.WORKING, model.step.value)
    }

    @Test
    fun `answers to a request the user has since replaced or skipped are dropped`() {
        model.begin(address)
        setup.requests[0].onFailure(CompanionSetup.Failure.NOT_FOUND)
        assertEquals(OfferStep.NOT_FOUND, model.step.value)
        model.begin(address)
        setup.requests[0].onAssociated()
        assertEquals(OfferStep.WORKING, model.step.value)
        model.skip()
        setup.requests[1].onAssociated()
        assertEquals(OfferStep.SKIPPED, model.step.value)
        assertEquals(emptyList<String>(), setup.observing)
    }

    @Test
    fun `the chooser's answer finishes the offer or offers it again`() {
        model.begin(address)
        model.onChooserResult(chosen = false, address)
        assertEquals(OfferStep.ASK, model.step.value)
        model.onPermission(granted = true, address)
        model.onChooserResult(chosen = true, address)
        assertEquals(OfferStep.DONE, model.step.value)
        assertEquals(listOf(address), setup.observing)
        assertEquals(2, setup.requests.size)
    }

    @Test
    fun `a refused permission asks again without a request`() {
        model.onPermission(granted = false, address)
        assertEquals(OfferStep.ASK, model.step.value)
        assertEquals(0, setup.requests.size)
    }
}
