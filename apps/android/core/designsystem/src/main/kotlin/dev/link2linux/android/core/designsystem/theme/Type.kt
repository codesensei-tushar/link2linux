package dev.link2linux.android.core.designsystem.theme

import androidx.compose.material3.Typography
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import dev.link2linux.android.core.designsystem.R

/**
 * Noto Sans, the font the desktop app gets from a stock GNOME system. The desktop's semibold
 * weights render bold there, so a regular and a bold file cover everything it shows.
 */
val NotoSans = FontFamily(
    Font(R.font.noto_sans_regular, FontWeight.Normal),
    Font(R.font.noto_sans_bold, FontWeight.Bold),
)

private val Base = Typography()

private fun TextStyle.noto() = copy(fontFamily = NotoSans)

private fun style(size: Int, lineHeight: Int, weight: FontWeight, tight: Boolean = false) = TextStyle(
    fontFamily = NotoSans,
    fontWeight = weight,
    fontSize = size.sp,
    lineHeight = lineHeight.sp,
    letterSpacing = if (tight) (-0.01).em else 0.em,
)

// The desktop's scale: bold, slightly tightened titles over regular body text, and bold labels.
// Sizes are a step up from the desktop's where a phone is read at arm's length.
internal val L2lTypography = Typography(
    displayLarge = Base.displayLarge.noto(),
    displayMedium = Base.displayMedium.noto(),
    displaySmall = Base.displaySmall.noto(),
    headlineLarge = style(size = 28, lineHeight = 34, weight = FontWeight.Bold, tight = true),
    headlineMedium = style(size = 24, lineHeight = 30, weight = FontWeight.Bold, tight = true),
    headlineSmall = style(size = 20, lineHeight = 26, weight = FontWeight.Bold, tight = true),
    titleLarge = style(size = 22, lineHeight = 28, weight = FontWeight.Bold, tight = true),
    titleMedium = style(size = 17, lineHeight = 24, weight = FontWeight.Bold),
    titleSmall = style(size = 15, lineHeight = 20, weight = FontWeight.Bold),
    bodyLarge = style(size = 16, lineHeight = 24, weight = FontWeight.Normal),
    bodyMedium = style(size = 14, lineHeight = 21, weight = FontWeight.Normal),
    bodySmall = style(size = 12, lineHeight = 18, weight = FontWeight.Normal),
    labelLarge = style(size = 15, lineHeight = 20, weight = FontWeight.Bold),
    labelMedium = style(size = 12, lineHeight = 16, weight = FontWeight.Bold),
    labelSmall = style(size = 11, lineHeight = 16, weight = FontWeight.Bold),
)
