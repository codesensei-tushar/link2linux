package dev.link2linux.android.ui

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.lifecycle.viewmodel.navigation3.rememberViewModelStoreNavEntryDecorator
import androidx.navigation3.runtime.NavKey
import androidx.navigation3.runtime.entryProvider
import androidx.navigation3.runtime.rememberNavBackStack
import androidx.navigation3.runtime.rememberSaveableStateHolderNavEntryDecorator
import androidx.navigation3.ui.NavDisplay
import dev.link2linux.android.core.model.Feature
import dev.link2linux.android.feature.calls.CallsRoute
import dev.link2linux.android.feature.calls.callsEntry
import dev.link2linux.android.feature.clipboard.ClipboardRoute
import dev.link2linux.android.feature.clipboard.SendClipboardButton
import dev.link2linux.android.feature.clipboard.clipboardEntry
import dev.link2linux.android.feature.files.FilesRoute
import dev.link2linux.android.feature.files.filesEntry
import dev.link2linux.android.feature.messages.MessagesRoute
import dev.link2linux.android.feature.messages.messagesEntry
import dev.link2linux.android.feature.notifications.NotificationsRoute
import dev.link2linux.android.feature.notifications.notificationsEntry
import dev.link2linux.android.feature.pairing.PairingRoute
import dev.link2linux.android.feature.pairing.pairingEntry
import dev.link2linux.android.feature.photos.PhotosRoute
import dev.link2linux.android.feature.photos.photosEntry
import dev.link2linux.android.feature.screen.ScreenRoute
import dev.link2linux.android.feature.screen.screenEntry

/** The route that opens each opt-in feature, in the order the features are listed. */
internal val featureRoutes: Map<Feature, NavKey> = linkedMapOf(
    Feature.NOTIFICATIONS to NotificationsRoute,
    Feature.MESSAGES to MessagesRoute,
    Feature.CALLS to CallsRoute,
    Feature.PHOTOS to PhotosRoute,
    Feature.FILES to FilesRoute,
    Feature.CLIPBOARD to ClipboardRoute,
    Feature.SCREEN to ScreenRoute,
)

/** Root of the UI: one back stack of feature routes, starting at pairing. */
@Composable
fun L2lApp(modifier: Modifier = Modifier) {
    val backStack = rememberNavBackStack(PairingRoute)
    val goBack: () -> Unit = { backStack.removeLastOrNull() }

    NavDisplay(
        backStack = backStack,
        modifier = modifier,
        onBack = goBack,
        entryDecorators = listOf(
            rememberSaveableStateHolderNavEntryDecorator(),
            rememberViewModelStoreNavEntryDecorator(),
        ),
        entryProvider = entryProvider {
            pairingEntry(
                features = featureRoutes.keys.toList(),
                onOpenFeature = { feature -> backStack.add(featureRoutes.getValue(feature)) },
                connectedActions = { SendClipboardButton() },
            )
            notificationsEntry(onBack = goBack)
            messagesEntry(onBack = goBack)
            callsEntry(onBack = goBack)
            photosEntry(onBack = goBack)
            filesEntry(onBack = goBack)
            clipboardEntry(onBack = goBack)
            screenEntry(onBack = goBack)
        },
    )
}
