package dev.link2linux.android.feature.notifications

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.drawable.Drawable
import android.graphics.drawable.Icon
import dev.link2linux.android.core.notifications.EncodedImage
import dev.link2linux.android.core.notifications.ImageKind
import dev.link2linux.android.core.notifications.ImageSource
import dev.link2linux.android.core.notifications.Limits
import dev.link2linux.protocol.v1.ImageFormat
import java.io.ByteArrayOutputStream

/** Renders an Android drawable or bitmap to the sizes and formats of the protocol. */
internal object ImageRendering {
    fun encode(drawable: Drawable?, kind: ImageKind): EncodedImage? {
        drawable ?: return null
        val w = drawable.intrinsicWidth.takeIf { it > 0 } ?: kind.maxEdge
        val h = drawable.intrinsicHeight.takeIf { it > 0 } ?: kind.maxEdge
        val (tw, th) = target(w, h, kind)
        val bitmap = Bitmap.createBitmap(tw, th, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        drawable.setBounds(0, 0, tw, th)
        drawable.draw(canvas)
        return encode(bitmap, kind).also { bitmap.recycle() }
    }

    fun encode(source: Bitmap?, kind: ImageKind): EncodedImage? {
        source ?: return null
        if (source.width <= 0 || source.height <= 0) return null
        val soft = (if (source.config == Bitmap.Config.HARDWARE) source.copy(Bitmap.Config.ARGB_8888, false) else source)
            ?: return null
        val (tw, th) = target(soft.width, soft.height, kind)
        val scaled = if (tw == soft.width && th == soft.height) soft else Bitmap.createScaledBitmap(soft, tw, th, true)
        try {
            return compress(scaled, kind)
        } finally {
            if (scaled !== source) scaled.recycle()
            if (soft !== source && soft !== scaled) soft.recycle()
        }
    }

    /** App icons are exactly 128 by 128; the others keep their shape and are never enlarged. */
    private fun target(w: Int, h: Int, kind: ImageKind): Pair<Int, Int> {
        if (kind == ImageKind.APP_ICON) return kind.maxEdge to kind.maxEdge
        val longest = maxOf(w, h)
        if (longest <= kind.maxEdge) return w to h
        val scale = kind.maxEdge.toFloat() / longest
        return maxOf(1, (w * scale).toInt()) to maxOf(1, (h * scale).toInt())
    }

    private fun compress(bitmap: Bitmap, kind: ImageKind): EncodedImage? {
        val out = ByteArrayOutputStream()
        if (kind == ImageKind.PICTURE) {
            // JPEG has no alpha; pictures are photos. Step the quality down until it fits comfortably.
            for (quality in intArrayOf(85, 70, 55, 40)) {
                out.reset()
                bitmap.compress(Bitmap.CompressFormat.JPEG, quality, out)
                if (out.size() <= PICTURE_TARGET_BYTES) break
            }
            if (out.size() > Limits.IMAGE_BYTES) return null
            return EncodedImage(ImageFormat.IMAGE_FORMAT_JPEG, out.toByteArray(), bitmap.width, bitmap.height)
        }
        bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
        if (out.size() > Limits.IMAGE_BYTES) return null
        return EncodedImage(ImageFormat.IMAGE_FORMAT_PNG, out.toByteArray(), bitmap.width, bitmap.height)
    }

    private const val PICTURE_TARGET_BYTES = 700 * 1024
}

/** An icon of a notification, loaded and encoded only when the engine asks. */
internal class IconImageSource(
    private val context: Context,
    private val icon: Icon,
    override val cacheKey: String? = null,
) : ImageSource {
    override fun render(kind: ImageKind): EncodedImage? =
        ImageRendering.encode(icon.loadDrawable(context), kind)
}

internal class BitmapImageSource(private val bitmap: Bitmap) : ImageSource {
    override fun render(kind: ImageKind): EncodedImage? = ImageRendering.encode(bitmap, kind)
}

/** The posting app's icon, cached by package so every notification of the app shares one image. */
internal class DrawableImageSource(
    private val load: () -> Drawable?,
    override val cacheKey: String,
) : ImageSource {
    override fun render(kind: ImageKind): EncodedImage? = ImageRendering.encode(load(), kind)
}
