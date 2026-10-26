package dev.link2linux.android.feature.notifications

import android.content.ContentResolver
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.drawable.Drawable
import android.graphics.drawable.Icon
import android.net.Uri
import android.os.Build
import dev.link2linux.android.core.notifications.EncodedImage
import dev.link2linux.android.core.notifications.ImageKind
import dev.link2linux.android.core.notifications.ImageSource
import dev.link2linux.android.core.notifications.Limits
import dev.link2linux.protocol.v1.ImageFormat
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

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

/**
 * An icon of a notification, loaded and encoded only when the engine asks.
 * An icon that points at a URI is read by [UriImages], within limits;
 * Android's own loading would read and decode whatever the URI serves.
 */
internal class IconImageSource(
    private val context: Context,
    private val icon: Icon,
    override val cacheKey: String? = null,
) : ImageSource {
    override fun render(kind: ImageKind): EncodedImage? = when (IconRoute.of(typeOf(icon))) {
        IconRoute.URI -> uriOf(icon)?.let { UriImages.load(context, it, kind) }
        IconRoute.LOAD -> ImageRendering.encode(icon.loadDrawable(context), kind)
        IconRoute.SKIP -> null
    }

    private companion object {
        // Android 8 has getType and getUri too, just not public; it restricts no hidden methods yet.
        fun typeOf(icon: Icon): Int? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            icon.type
        } else {
            runCatching { Icon::class.java.getMethod("getType").invoke(icon) as Int }.getOrNull()
        }

        fun uriOf(icon: Icon): Uri? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            icon.uri
        } else {
            runCatching { Icon::class.java.getMethod("getUri").invoke(icon) as Uri }.getOrNull()
        }
    }
}

/** How an icon is rendered, by its `Icon` type. */
internal enum class IconRoute {
    /** Read through [UriImages], within limits. */
    URI,

    /** Loaded by Android: the pixels are in the notification or in the posting app's package. */
    LOAD,

    /** Not rendered: the type could not be read, so the icon might be a URI. */
    SKIP,
    ;

    companion object {
        /** `Icon.TYPE_URI_ADAPTIVE_BITMAP`, from Android 11. */
        private const val TYPE_URI_ADAPTIVE_BITMAP = 6

        /** [type] is null when it could not be read. */
        fun of(type: Int?): IconRoute = when (type) {
            null -> SKIP
            Icon.TYPE_URI, TYPE_URI_ADAPTIVE_BITMAP -> URI
            else -> LOAD
        }
    }
}

/**
 * Reads an image another app serves from a URI, within limits: at most
 * [MAX_BYTES] in [DEADLINE_MILLIS], and decoded no larger than needed. Any
 * app can post a notification whose icon is a URI that never stops sending,
 * or a small file that claims huge dimensions.
 */
internal object UriImages {
    private const val MAX_BYTES = 4 * 1024 * 1024
    private const val DEADLINE_MILLIS = 2_000L

    private val deadlines = Executors.newSingleThreadScheduledExecutor { task ->
        Thread(task, "l2l-image-deadline").apply { isDaemon = true }
    }

    fun load(context: Context, uri: Uri, kind: ImageKind): EncodedImage? {
        // Only what a provider or a package's resources serve: never a file, which this app would open as itself.
        if (uri.scheme != ContentResolver.SCHEME_CONTENT && uri.scheme != ContentResolver.SCHEME_ANDROID_RESOURCE) return null
        val bytes = read(context, uri) ?: return null
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
        val options = BitmapFactory.Options().apply { inSampleSize = sampleSize(bounds.outWidth, bounds.outHeight, kind.maxEdge) }
        val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options) ?: return null
        return try {
            ImageRendering.encode(bitmap, kind)
        } finally {
            bitmap.recycle()
        }
    }

    /** The largest power of two that still leaves the longer edge at least [maxEdge]: decoding then never holds more than about four times the pixels sent. */
    fun sampleSize(width: Int, height: Int, maxEdge: Int): Int {
        var sample = 1
        while (maxOf(width, height) / (sample * 2) >= maxEdge) sample *= 2
        return sample
    }

    /** The bytes at [uri], or null if there are more than [MAX_BYTES] or they take longer than [DEADLINE_MILLIS]. */
    private fun read(context: Context, uri: Uri): ByteArray? {
        val stream = context.contentResolver.openInputStream(uri) ?: return null
        // Closing the stream ends a read that waits forever.
        val deadline = deadlines.schedule({ runCatching { stream.close() } }, DEADLINE_MILLIS, TimeUnit.MILLISECONDS)
        return try {
            stream.use(::readAtMost)
        } catch (_: IOException) {
            null
        } finally {
            deadline.cancel(false)
        }
    }

    private fun readAtMost(input: InputStream): ByteArray? {
        val out = ByteArrayOutputStream()
        val buffer = ByteArray(16 * 1024)
        while (true) {
            val read = input.read(buffer)
            if (read < 0) return out.toByteArray()
            out.write(buffer, 0, read)
            if (out.size() > MAX_BYTES) return null
        }
    }
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
