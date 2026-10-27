package dev.link2linux.android.core.designsystem.theme

import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

// The desktop app's palette (apps/desktop/src/styles/tokens.css), so both apps look like one
// product. Keep the two in step: a color changed there changes here too.

/** Colors the desktop has and Material 3 has no role for: status tones and borders. */
@Immutable
data class L2lColors(
    val border: Color,
    val borderStrong: Color,
    val textSubtle: Color,
    val accentSoft: Color,
    val success: Color,
    val successSoft: Color,
    val warning: Color,
    val warningSoft: Color,
    val danger: Color,
    val dangerSoft: Color,
)

internal val LightL2lColors = L2lColors(
    border = Color(0xFFDFE2EB),
    borderStrong = Color(0xFFC7CCDA),
    textSubtle = Color(0xFF6A7085),
    accentSoft = Color(0xFFE8E8FB),
    success = Color(0xFF0B7A6D),
    successSoft = Color(0xFFDCF3EF),
    warning = Color(0xFF9A5B00),
    warningSoft = Color(0xFFFBEFD9),
    danger = Color(0xFFB83232),
    dangerSoft = Color(0xFFFBE3E3),
)

internal val DarkL2lColors = L2lColors(
    border = Color(0xFF2A2F3D),
    borderStrong = Color(0xFF3A4052),
    textSubtle = Color(0xFF868DA2),
    accentSoft = Color(0xFF262852),
    success = Color(0xFF4FD1BD),
    successSoft = Color(0xFF123430),
    warning = Color(0xFFF0B45A),
    warningSoft = Color(0xFF3A2A12),
    danger = Color(0xFFF28B8B),
    dangerSoft = Color(0xFF3D1C1F),
)

internal val LocalL2lColors = staticCompositionLocalOf { LightL2lColors }

// Pages are the desktop's background color and cards are its surface color, so "surface" is the
// page and the container roles carry the card white. A transparent tint keeps elevated surfaces
// flat, as on the desktop, instead of tinting them with the accent.

internal val LightColors = lightColorScheme(
    primary = Color(0xFF4747C9),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = LightL2lColors.accentSoft,
    onPrimaryContainer = Color(0xFF3A3AB0),
    inversePrimary = Color(0xFF8F90F7),
    secondary = Color(0xFF545B6E),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFF0F1F6),
    onSecondaryContainer = Color(0xFF161925),
    tertiary = LightL2lColors.success,
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = LightL2lColors.successSoft,
    onTertiaryContainer = LightL2lColors.success,
    error = LightL2lColors.danger,
    onError = Color(0xFFFFFFFF),
    errorContainer = LightL2lColors.dangerSoft,
    onErrorContainer = LightL2lColors.danger,
    background = Color(0xFFF5F6FA),
    onBackground = Color(0xFF161925),
    surface = Color(0xFFF5F6FA),
    onSurface = Color(0xFF161925),
    surfaceVariant = Color(0xFFF0F1F6),
    onSurfaceVariant = Color(0xFF545B6E),
    surfaceTint = Color.Transparent,
    inverseSurface = Color(0xFF161925),
    inverseOnSurface = Color(0xFFF5F6FA),
    outline = LightL2lColors.borderStrong,
    outlineVariant = LightL2lColors.border,
    surfaceBright = Color(0xFFFFFFFF),
    surfaceDim = Color(0xFFECEEF5),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceContainerLow = Color(0xFFFFFFFF),
    surfaceContainer = Color(0xFFFFFFFF),
    surfaceContainerHigh = Color(0xFFFFFFFF),
    surfaceContainerHighest = Color(0xFFECEEF5),
)

internal val DarkColors = darkColorScheme(
    primary = Color(0xFF8F90F7),
    onPrimary = Color(0xFF11122A),
    primaryContainer = DarkL2lColors.accentSoft,
    onPrimaryContainer = Color(0xFFA7A8FA),
    inversePrimary = Color(0xFF4747C9),
    secondary = Color(0xFFA6ACBE),
    onSecondary = Color(0xFF101219),
    secondaryContainer = Color(0xFF1F2330),
    onSecondaryContainer = Color(0xFFE7E9F1),
    tertiary = DarkL2lColors.success,
    onTertiary = Color(0xFF101219),
    tertiaryContainer = DarkL2lColors.successSoft,
    onTertiaryContainer = DarkL2lColors.success,
    error = DarkL2lColors.danger,
    onError = Color(0xFF101219),
    errorContainer = DarkL2lColors.dangerSoft,
    onErrorContainer = DarkL2lColors.danger,
    background = Color(0xFF101219),
    onBackground = Color(0xFFE7E9F1),
    surface = Color(0xFF101219),
    onSurface = Color(0xFFE7E9F1),
    surfaceVariant = Color(0xFF1F2330),
    onSurfaceVariant = Color(0xFFA6ACBE),
    surfaceTint = Color.Transparent,
    inverseSurface = Color(0xFFE7E9F1),
    inverseOnSurface = Color(0xFF101219),
    outline = DarkL2lColors.borderStrong,
    outlineVariant = DarkL2lColors.border,
    surfaceBright = Color(0xFF1F2330),
    surfaceDim = Color(0xFF0B0D13),
    surfaceContainerLowest = Color(0xFF181B25),
    surfaceContainerLow = Color(0xFF181B25),
    surfaceContainer = Color(0xFF181B25),
    surfaceContainerHigh = Color(0xFF181B25),
    surfaceContainerHighest = Color(0xFF1F2330),
)
