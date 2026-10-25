package dev.link2linux.android.ui

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.lifecycle.viewmodel.navigation3.rememberViewModelStoreNavEntryDecorator
import androidx.navigation3.runtime.entryProvider
import androidx.navigation3.runtime.rememberNavBackStack
import androidx.navigation3.runtime.rememberSaveableStateHolderNavEntryDecorator
import androidx.navigation3.ui.NavDisplay
import dev.link2linux.android.feature.pairing.PairingRoute
import dev.link2linux.android.feature.pairing.pairingEntry

/** Root of the UI: one back stack of feature routes, starting at pairing. */
@Composable
fun L2lApp(modifier: Modifier = Modifier) {
    val backStack = rememberNavBackStack(PairingRoute)

    NavDisplay(
        backStack = backStack,
        modifier = modifier,
        onBack = { backStack.removeLastOrNull() },
        entryDecorators = listOf(
            rememberSaveableStateHolderNavEntryDecorator(),
            rememberViewModelStoreNavEntryDecorator(),
        ),
        entryProvider = entryProvider {
            pairingEntry()
        },
    )
}
