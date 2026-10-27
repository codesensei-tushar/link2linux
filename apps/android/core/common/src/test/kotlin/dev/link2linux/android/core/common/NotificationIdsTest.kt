package dev.link2linux.android.core.common

import org.junit.Assert.assertEquals
import org.junit.Test

class NotificationIdsTest {
    @Test
    fun `untagged notifications each have their own id`() {
        assertEquals(NotificationIds.untagged.size, NotificationIds.untagged.toSet().size)
    }

    @Test
    fun `every declared id is registered`() {
        val declared = NotificationIds::class.java.declaredFields
            .filter { it.type == Int::class.javaPrimitiveType && java.lang.reflect.Modifier.isStatic(it.modifiers) }
            .map { it.getInt(null) }
            .toSet()
        assertEquals(declared, NotificationIds.untagged.toSet())
    }
}
