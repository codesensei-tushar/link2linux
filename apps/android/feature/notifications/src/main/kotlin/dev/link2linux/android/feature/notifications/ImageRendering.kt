package dev.link2linux.android.feature.notifications

import android.content.ContentResolver
import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.res.Resources
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.drawable.Drawable
import android.graphics.drawable.Icon
import android.net.Uri
import android.os.Build
import android.os.Parcel
import android.util.DisplayMetrics
import android.util.TypedValue
import dev.link2linux.android.core.notifications.EncodedImage
import dev.link2linux.android.core.notifications.ImageKind
import dev.link2linux.android.core.notifications.ImageSource
import dev.link2linux.android.core.notifications.Limits
import dev.link2linux.protocol.v1.ImageFormat
import java.io.ByteArrayOutputStream
import kotlin.math.ceil
import org.xmlpull.v1.XmlPullParser

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
 * Any app can post an icon that is slow to read or huge once decoded, so
 * Android's own loading is used only for a bitmap the notification already
 * holds, and for a drawable whose bitmaps were checked first.
 */
internal class IconImageSource(
    private val context: Context,
    private val icon: Icon,
    override val cacheKey: String? = null,
) : ImageSource {
    override fun render(kind: ImageKind): EncodedImage? = when (IconRoute.of(typeOf(icon))) {
        IconRoute.URI -> uriOf(icon)?.let { UriImages.load(context, it, kind) }
        IconRoute.BITMAP -> ImageRendering.encode(icon.loadDrawable(context), kind)
        IconRoute.DATA -> dataOf(icon)?.let { BoundedImages.decode(it, kind) }
        IconRoute.RESOURCE -> resourceOf(icon)?.let { (pkg, id) ->
            ResourceImages.load(resourcesOf(pkg), id, kind) { icon.loadDrawable(context) }
        }
        IconRoute.SKIP -> null
    }

    /** As Android finds an icon's package: none is this app, "android" is the system. */
    private fun resourcesOf(pkg: String?): Resources = when {
        pkg.isNullOrEmpty() -> context.resources
        pkg == "android" -> Resources.getSystem()
        else -> context.packageManager.getResourcesForApplication(pkg)
    }

    private companion object {
        // Android 8 has getType, getUri, getResPackage and getResId too, just not public; it restricts no hidden methods yet.
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

        /** The package and the id of a resource icon, or null if they cannot be read. */
        fun resourceOf(icon: Icon): Pair<String?, Int>? {
            val (pkg, id) = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                icon.resPackage to icon.resId
            } else {
                runCatching {
                    Icon::class.java.getMethod("getResPackage").invoke(icon) as String? to
                        Icon::class.java.getMethod("getResId").invoke(icon) as Int
                }.getOrNull() ?: return null
            }
            return if (id == 0) null else pkg to id
        }

        /**
         * The compressed image of a data icon, which Android would decode at
         * full size. Only Android 13 and later have a public way to read it:
         * the icon written to a parcel, as its type, the length, then the bytes.
         * Before that, a data icon is not rendered.
         */
        fun dataOf(icon: Icon): ByteArray? {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return null
            val parcel = Parcel.obtain()
            try {
                icon.writeToParcel(parcel, 0)
                parcel.setDataPosition(0)
                if (parcel.readInt() != Icon.TYPE_DATA) return null
                val length = parcel.readInt()
                return parcel.readBlob()?.takeIf { it.size == length }
            } finally {
                parcel.recycle()
            }
        }
    }
}

/** How an icon is rendered, by its `Icon` type. */
internal enum class IconRoute {
    /** Read through [UriImages], within limits. */
    URI,

    /** Drawn by Android: the notification holds the bitmap already. */
    BITMAP,

    /** Compressed bytes the notification holds, decoded by [BoundedImages]. */
    DATA,

    /** A drawable of a package, loaded by [ResourceImages]. */
    RESOURCE,

    /** Not rendered: the type could not be read, so the icon might be a URI, or it is one this app does not know. */
    SKIP,
    ;

    companion object {
        /** `Icon.TYPE_URI_ADAPTIVE_BITMAP`, from Android 11. */
        private const val TYPE_URI_ADAPTIVE_BITMAP = 6

        /** [type] is null when it could not be read. */
        fun of(type: Int?): IconRoute = when (type) {
            Icon.TYPE_URI, TYPE_URI_ADAPTIVE_BITMAP -> URI
            Icon.TYPE_BITMAP, Icon.TYPE_ADAPTIVE_BITMAP -> BITMAP
            Icon.TYPE_DATA -> DATA
            Icon.TYPE_RESOURCE -> RESOURCE
            else -> SKIP
        }
    }
}

/**
 * Decodes an image another app supplied no larger than needed: at most
 * [MAX_BYTES] of it, and with its size checked before the pixels are
 * decoded, since a small file can claim 20000 by 20000 pixels.
 */
internal object BoundedImages {
    const val MAX_BYTES = 4 * 1024 * 1024

    fun decode(bytes: ByteArray, kind: ImageKind): EncodedImage? {
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
}

/**
 * Reads an image another app serves from a URI, within limits: at most
 * [BoundedImages.MAX_BYTES] in [DEADLINE_MILLIS], opening included, and
 * decoded no larger than needed. Any app can post a notification whose icon
 * is a URI that never opens or never stops sending, or a small file that
 * claims huge dimensions.
 */
internal object UriImages {
    private const val DEADLINE_MILLIS = 2_000L

    // A provider that never answers keeps a thread each time; two per provider and eight in all at most.
    private val reader = DeadlineReader(BoundedImages.MAX_BYTES, DEADLINE_MILLIS, maxReads = 8, maxPerSource = 2)

    fun load(context: Context, uri: Uri, kind: ImageKind): EncodedImage? {
        // Only what a provider or a package's resources serve: never a file, which this app would open as itself.
        if (uri.scheme != ContentResolver.SCHEME_CONTENT && uri.scheme != ContentResolver.SCHEME_ANDROID_RESOURCE) return null
        val bytes = reader.read(uri.authority.orEmpty()) { context.contentResolver.openInputStream(uri) } ?: return null
        return BoundedImages.decode(bytes, kind)
    }
}

/**
 * Loads a drawable of a package within limits. A bitmap is decoded by
 * [BoundedImages]. A drawable defined in XML (a vector, a layer list, an
 * adaptive icon) is left to Android, which decodes every bitmap it uses at
 * full size, only when those bitmaps were checked first: at most
 * [MAX_XML_PIXELS] in all, at the size Android scales them to.
 */
internal object ResourceImages {
    private const val MAX_XML_PIXELS = 8L * 1024 * 1024
    private const val MAX_XML_FILES = 32
    private const val MAX_XML_DEPTH = 4
    private val BITMAP_TYPES = setOf("drawable", "mipmap")

    /** [android] loads the drawable as Android does, for one defined in XML or by a color. */
    fun load(res: Resources, id: Int, kind: ImageKind, android: () -> Drawable?): EncodedImage? {
        val value = TypedValue()
        res.getValue(id, value, true)
        val file = value.string?.toString()
        return when {
            // A color.
            file == null -> ImageRendering.encode(android(), kind)
            file.endsWith(".xml") -> if (xmlFits(res, id)) ImageRendering.encode(android(), kind) else null
            else -> res.openRawResource(id).use { readAtMost(it, BoundedImages.MAX_BYTES) }?.let { BoundedImages.decode(it, kind) }
        }
    }

    /** The pixels Android decodes a resource bitmap to: from the [density] of its folder to the screen's [targetDensity]. */
    fun scaledPixels(width: Int, height: Int, density: Int, targetDensity: Int): Long {
        val from = when (density) {
            TypedValue.DENSITY_NONE -> return width.toLong() * height
            TypedValue.DENSITY_DEFAULT -> DisplayMetrics.DENSITY_DEFAULT
            else -> density
        }
        val scale = targetDensity.coerceAtLeast(1).toDouble() / from
        return ceil(width * scale).toLong() * ceil(height * scale).toLong()
    }

    /** Whether every bitmap the XML drawable [id] uses, through other XML too, fits in [MAX_XML_PIXELS] together. */
    private fun xmlFits(res: Resources, id: Int): Boolean {
        var files = 0
        var pixels = 0L

        fun fits(id: Int, depth: Int): Boolean {
            if (++files > MAX_XML_FILES || depth > MAX_XML_DEPTH) return false
            val value = TypedValue()
            res.getValue(id, value, true)
            val file = value.string?.toString() ?: return true // a color
            if (!file.endsWith(".xml")) {
                val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
                res.openRawResource(id, value).use { BitmapFactory.decodeStream(it, null, bounds) }
                if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return false
                pixels += scaledPixels(bounds.outWidth, bounds.outHeight, value.density, res.displayMetrics.densityDpi)
                return pixels <= MAX_XML_PIXELS
            }
            val parser = res.getXml(id)
            try {
                while (parser.next() != XmlPullParser.END_DOCUMENT) {
                    if (parser.eventType != XmlPullParser.START_TAG) continue
                    for (i in 0 until parser.attributeCount) {
                        val ref = parser.getAttributeResourceValue(i, 0)
                        if (ref != 0 && res.getResourceTypeName(ref) in BITMAP_TYPES && !fits(ref, depth + 1)) return false
                    }
                }
            } finally {
                parser.close()
            }
            return true
        }

        return fits(id, 0)
    }
}

internal class BitmapImageSource(private val bitmap: Bitmap) : ImageSource {
    override fun render(kind: ImageKind): EncodedImage? = ImageRendering.encode(bitmap, kind)
}

/** The posting app's icon, cached by package so every notification of the app shares one image. */
internal class AppIconImageSource(
    private val context: Context,
    private val info: ApplicationInfo,
    override val cacheKey: String,
) : ImageSource {
    override fun render(kind: ImageKind): EncodedImage? {
        val pm = context.packageManager
        // No icon of its own: Android's default one.
        if (info.icon == 0) return ImageRendering.encode(info.loadIcon(pm), kind)
        return ResourceImages.load(pm.getResourcesForApplication(info), info.icon, kind) { info.loadIcon(pm) }
    }
}
