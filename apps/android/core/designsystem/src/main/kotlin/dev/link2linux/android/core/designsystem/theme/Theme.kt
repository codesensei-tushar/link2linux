package dev.link2linux.android.core.designsystem.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.ReadOnlyComposable

/**
 * The link2linux theme: the desktop app's colors, font and shapes on Material 3. Follows the
 * system light/dark setting. Wallpaper colors are left out on purpose, so the phone app looks
 * like the desktop app it pairs with.
 */
@Composable
fun L2lTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    CompositionLocalProvider(LocalL2lColors provides if (darkTheme) DarkL2lColors else LightL2lColors) {
        MaterialTheme(
            colorScheme = if (darkTheme) DarkColors else LightColors,
            typography = L2lTypography,
            shapes = L2lShapes,
            content = content,
        )
    }
}

/** The theme's colors that Material 3 has no role for. */
object L2lTheme {
    val colors: L2lColors
        @Composable
        @ReadOnlyComposable
        get() = LocalL2lColors.current
}
