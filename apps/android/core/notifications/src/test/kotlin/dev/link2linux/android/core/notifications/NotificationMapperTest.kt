package dev.link2linux.android.core.notifications

import dev.link2linux.protocol.v1.ImageFormat
import dev.link2linux.protocol.v1.NotificationActionKind
import dev.link2linux.protocol.v1.NotificationCategory
import dev.link2linux.protocol.v1.NotificationContent
import dev.link2linux.protocol.v1.NotificationImportance
import dev.link2linux.protocol.v1.NotificationVisibility
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationMapperTest {
    private val images = ImageStore()
    private val mapper = NotificationMapper(images, RedactionDetector(appLocked = setOf("App is locked")))

    private fun action(
        label: String,
        semantic: Int = AndroidValues.SEMANTIC_NONE,
        reply: ReplyInput? = null,
        replyKey: String? = null,
        role: CallRole? = null,
        unlock: Boolean = false,
    ) = ActionSnapshot(
        label = label,
        semanticAction = semantic,
        replyKey = replyKey,
        reply = reply,
        requiresUnlock = unlock,
        callRole = role,
        invoker = { _, _ -> ActionOutcome.SENT },
    )

    private fun png(seed: Int) = object : ImageSource {
        override fun render(kind: ImageKind) =
            EncodedImage(ImageFormat.IMAGE_FORMAT_PNG, ByteArray(10) { (it + seed).toByte() }, 4, 4)
    }

    private val base = NotificationSnapshot(
        key = "0|org.chat|1|null|10001",
        packageName = "org.chat",
        appName = "Chat",
        postTimeMillis = 1_000,
        title = "Ann",
        text = "Hi",
        importance = AndroidValues.IMPORTANCE_HIGH,
    )

    @Test
    fun `basic fields map across`() {
        val m = mapper.map(
            base.copy(
                subText = "ann@example.org",
                category = "msg",
                visibility = AndroidValues.VISIBILITY_PUBLIC,
                isOngoing = false,
                eventTimeMillis = 2_000,
                appIcon = png(1),
            ),
        ).notification
        assertEquals("0|org.chat|1|null|10001", m.key)
        assertEquals("org.chat", m.appId)
        assertEquals("Chat", m.appName)
        assertEquals("Ann", m.title)
        assertEquals("Hi", m.text)
        assertEquals("ann@example.org", m.subText)
        assertEquals(2_000L, m.whenUnixMs)
        assertEquals(NotificationCategory.NOTIFICATION_CATEGORY_MESSAGE, m.category)
        assertEquals(NotificationImportance.NOTIFICATION_IMPORTANCE_HIGH, m.importance)
        assertEquals(NotificationVisibility.NOTIFICATION_VISIBILITY_PUBLIC, m.visibility)
        assertEquals(NotificationContent.NOTIFICATION_CONTENT_FULL, m.content)
        assertTrue(m.dismissible)
        assertEquals(4, m.appIcon.width)
        assertEquals(0L, m.revision)
    }

    @Test
    fun `the post time is used when the app set no event time`() {
        assertEquals(1_000L, mapper.map(base).notification.whenUnixMs)
    }

    @Test
    fun `expanded text wins and long text is cut`() {
        val m = mapper.map(base.copy(bigTitle = "Big", bigText = "x".repeat(9000))).notification
        assertEquals("Big", m.title)
        assertEquals(8 * 1024, m.text.toByteArray().size)
    }

    @Test
    fun `long keys and groups are hashed`() {
        val m = mapper.map(base.copy(key = "k".repeat(400), groupKey = "g".repeat(400))).notification
        assertTrue(m.key.startsWith("h:"))
        assertTrue(m.group.startsWith("h:"))
    }

    @Test
    fun `ongoing and uncancellable notifications are not dismissible`() {
        assertFalse(mapper.map(base.copy(isOngoing = true)).notification.dismissible)
        assertFalse(mapper.map(base.copy(isClearable = false)).notification.dismissible)
        assertTrue(mapper.map(base.copy(isOngoing = true)).notification.ongoing)
    }

    @Test
    fun `conversations keep the latest ten messages oldest first`() {
        val messages = (1..12).map { MessageSnapshot("S$it", "m$it", it * 100L, outgoing = it == 12) }
        val m = mapper.map(
            base.copy(conversation = ConversationSnapshot(messages, title = "Team", isGroup = true)),
        ).notification
        assertEquals(10, m.messagesCount)
        assertEquals("m3", m.messagesList.first().text)
        assertEquals("m12", m.messagesList.last().text)
        assertEquals("", m.messagesList.last().sender)
        assertTrue(m.messagesList.last().outgoing)
        assertEquals("S3", m.messagesList.first().sender)
        assertEquals("Team", m.conversationTitle)
        assertTrue(m.isGroupConversation)
        assertEquals(NotificationCategory.NOTIFICATION_CATEGORY_MESSAGE, m.category)
    }

    @Test
    fun `a one to one conversation has no conversation title`() {
        val m = mapper.map(
            base.copy(
                text = "",
                conversation = ConversationSnapshot(listOf(MessageSnapshot("Ann", "latest", 5, false)), title = "x"),
            ),
        ).notification
        assertEquals("", m.conversationTitle)
        assertFalse(m.isGroupConversation)
        assertEquals("latest", m.text)
    }

    @Test
    fun `progress maps`() {
        val m = mapper.map(base.copy(progress = ProgressSnapshot(30, 100, false))).notification
        assertEquals(30, m.progress.current)
        assertEquals(100, m.progress.max)
        assertTrue(mapper.map(base.copy(progress = ProgressSnapshot(0, 0, true))).notification.progress.indeterminate)
        assertEquals(
            NotificationCategory.NOTIFICATION_CATEGORY_PROGRESS,
            mapper.map(base.copy(progress = ProgressSnapshot(1, 2, false))).notification.category,
        )
    }

    @Test
    fun `categories`() {
        fun cat(c: String?) = mapper.map(base.copy(category = c)).notification.category
        assertEquals(NotificationCategory.NOTIFICATION_CATEGORY_CALL, cat("call"))
        assertEquals(NotificationCategory.NOTIFICATION_CATEGORY_MISSED_CALL, cat("missed_call"))
        assertEquals(NotificationCategory.NOTIFICATION_CATEGORY_MEDIA, cat("transport"))
        assertEquals(NotificationCategory.NOTIFICATION_CATEGORY_SYSTEM, cat("err"))
        assertEquals(NotificationCategory.NOTIFICATION_CATEGORY_UNSPECIFIED, cat(null))
    }

    @Test
    fun `importance maps and unknown values are unspecified`() {
        fun imp(i: Int) = mapper.map(base.copy(importance = i)).notification.importance
        assertEquals(NotificationImportance.NOTIFICATION_IMPORTANCE_MIN, imp(1))
        assertEquals(NotificationImportance.NOTIFICATION_IMPORTANCE_LOW, imp(2))
        assertEquals(NotificationImportance.NOTIFICATION_IMPORTANCE_DEFAULT, imp(3))
        assertEquals(NotificationImportance.NOTIFICATION_IMPORTANCE_HIGH, imp(5))
        assertEquals(NotificationImportance.NOTIFICATION_IMPORTANCE_UNSPECIFIED, imp(0))
    }

    // Actions

    @Test
    fun `action ids come from the label and semantic action, not the position`() {
        val a = action("Mark as read", AndroidValues.SEMANTIC_MARK_AS_READ)
        val b = action("Mute", AndroidValues.SEMANTIC_MUTE)
        val first = mapper.map(base.copy(actions = listOf(a, b))).notification
        val swapped = mapper.map(base.copy(actions = listOf(b, a))).notification
        val idsFirst = first.actionsList.associate { it.label to it.id }
        val idsSwapped = swapped.actionsList.associate { it.label to it.id }
        assertEquals(idsFirst, idsSwapped)
        assertNotEquals(first.actionsList[0].id, first.actionsList[1].id)
        assertEquals(NotificationActionKind.NOTIFICATION_ACTION_KIND_MARK_READ, first.actionsList[0].kind)
        assertEquals(NotificationActionKind.NOTIFICATION_ACTION_KIND_MUTE, first.actionsList[1].kind)
    }

    @Test
    fun `the same label with another semantic action gets another id`() {
        val m = mapper.map(
            base.copy(actions = listOf(action("Go", AndroidValues.SEMANTIC_DELETE), action("Go", AndroidValues.SEMANTIC_ARCHIVE))),
        ).notification
        assertNotEquals(m.actionsList[0].id, m.actionsList[1].id)
    }

    @Test
    fun `identical actions still get distinct ids`() {
        val m = mapper.map(base.copy(actions = listOf(action("Open"), action("Open")))).notification
        assertEquals(2, m.actionsList.map { it.id }.toSet().size)
        assertEquals(2, mapper.map(base.copy(actions = listOf(action("Open"), action("Open")))).actions.size)
    }

    @Test
    fun `a reply action exposes free text, choices and suggestions`() {
        val reply = action(
            "Reply",
            AndroidValues.SEMANTIC_REPLY,
            reply = ReplyInput(allowFreeForm = true, choices = listOf("Yes", "No"), allowGeneratedReplies = true),
            replyKey = "key_text_reply",
        )
        val m = mapper.map(base.copy(actions = listOf(reply), suggestedReplies = listOf("On my way", "Yes"))).notification
        val a = m.actionsList.single()
        assertEquals(NotificationActionKind.NOTIFICATION_ACTION_KIND_REPLY, a.kind)
        assertTrue(a.acceptsReply)
        assertEquals(listOf("Yes", "No", "On my way"), a.replyChoicesList)
    }

    @Test
    fun `suggestions are left out when the app does not allow generated replies`() {
        val reply = action("Reply", reply = ReplyInput(choices = listOf("Yes")), replyKey = "r")
        val m = mapper.map(base.copy(actions = listOf(reply), suggestedReplies = listOf("Sure"))).notification
        assertEquals(listOf("Yes"), m.actionsList.single().replyChoicesList)
    }

    @Test
    fun `choices only inputs do not accept free text and choices are limited to five`() {
        val reply = action("Reply", reply = ReplyInput(allowFreeForm = false, choices = (1..8).map { "c$it" }), replyKey = "r")
        val a = mapper.map(base.copy(actions = listOf(reply))).notification.actionsList.single()
        assertFalse(a.acceptsReply)
        assertEquals(5, a.replyChoicesCount)
    }

    @Test
    fun `requires unlock is copied`() {
        val a = mapper.map(base.copy(actions = listOf(action("Pay", unlock = true)))).notification.actionsList.single()
        assertTrue(a.requiresUnlock)
    }

    @Test
    fun `at most eight actions, and call actions are kept`() {
        val many = (1..10).map { action("A$it") } + action("", role = CallRole.DECLINE)
        val m = mapper.map(base.copy(actions = many)).notification
        assertEquals(8, m.actionsCount)
        assertEquals("call:decline", m.actionsList.first().id)
        assertEquals(NotificationActionKind.NOTIFICATION_ACTION_KIND_DECLINE_CALL, m.actionsList.first().kind)
        assertEquals("Decline", m.actionsList.first().label)
        assertEquals(NotificationCategory.NOTIFICATION_CATEGORY_CALL, m.category)
    }

    // Alerts

    @Test
    fun `only default and high importance alert`() {
        assertTrue(mapper.map(base.copy(importance = AndroidValues.IMPORTANCE_DEFAULT)).alertWhenNew)
        assertTrue(mapper.map(base).alertWhenNew)
        assertFalse(mapper.map(base.copy(importance = AndroidValues.IMPORTANCE_LOW)).alertWhenNew)
        assertFalse(mapper.map(base.copy(importance = AndroidValues.IMPORTANCE_MIN)).alertWhenNew)
    }

    @Test
    fun `only alert once and suppressed interruptions do not alert on update`() {
        val once = mapper.map(base.copy(onlyAlertOnce = true))
        assertTrue(once.alertWhenNew)
        assertFalse(once.alertOnUpdate)
        assertFalse(mapper.map(base.copy(interruptionSuppressed = true)).alertWhenNew)
    }

    @Test
    fun `group alert behaviour silences the summary or the children`() {
        val summarySilent = base.copy(groupKey = "g", isGroupSummary = true, groupAlertBehavior = AndroidValues.GROUP_ALERT_CHILDREN)
        val childSilent = base.copy(groupKey = "g", groupAlertBehavior = AndroidValues.GROUP_ALERT_SUMMARY)
        assertFalse(mapper.map(summarySilent).alertWhenNew)
        assertFalse(mapper.map(childSilent).alertWhenNew)
        assertTrue(mapper.map(childSilent.copy(groupAlertBehavior = AndroidValues.GROUP_ALERT_ALL)).alertWhenNew)
    }

    // Redaction and hiding

    @Test
    fun `system placeholders are detected and not sent`() {
        val m = mapper.map(
            base.copy(
                text = "Sensitive notification content hidden",
                largeIcon = png(2),
                actions = listOf(action("Reply", reply = ReplyInput(), replyKey = "r"), action("Archive")),
            ),
        )
        assertEquals(NotificationContent.NOTIFICATION_CONTENT_REDACTED_BY_SYSTEM, m.notification.content)
        assertEquals("", m.notification.text)
        assertEquals("Ann", m.notification.title)
        assertFalse(m.notification.hasLargeIcon())
        assertEquals(listOf("Archive"), m.notification.actionsList.map { it.label })
    }

    @Test
    fun `placeholder matching ignores case, spacing and a trailing period`() {
        val m = mapper.map(base.copy(text = "  sensitive  Notification content hidden. ")).notification
        assertEquals(NotificationContent.NOTIFICATION_CONTENT_REDACTED_BY_SYSTEM, m.content)
    }

    @Test
    fun `a locked app is told apart`() {
        val m = mapper.map(base.copy(bigText = "App is locked")).notification
        assertEquals(NotificationContent.NOTIFICATION_CONTENT_REDACTED_APP_LOCKED, m.content)
        assertEquals("", m.text)
    }

    @Test
    fun `placeholder inside a conversation message counts as redaction`() {
        val m = mapper.map(
            base.copy(conversation = ConversationSnapshot(listOf(MessageSnapshot("A", "Sensitive notification content hidden", 1, false)))),
        ).notification
        assertEquals(NotificationContent.NOTIFICATION_CONTENT_REDACTED_BY_SYSTEM, m.content)
        assertEquals(0, m.messagesCount)
    }

    @Test
    fun `ordinary text is full content`() {
        assertEquals(NotificationContent.NOTIFICATION_CONTENT_FULL, mapper.map(base).notification.content)
    }

    @Test
    fun `content hidden by the user keeps only the app, time and category`() {
        val m = mapper.map(
            base.copy(
                category = "msg",
                appIcon = png(1),
                largeIcon = png(2),
                subText = "sub",
                actions = listOf(action("Reply", reply = ReplyInput(), replyKey = "r"), action("Archive")),
                conversation = ConversationSnapshot(listOf(MessageSnapshot("A", "secret", 1, false))),
                progress = ProgressSnapshot(1, 2, false),
            ),
            contentHidden = true,
        )
        val n = m.notification
        assertEquals(NotificationContent.NOTIFICATION_CONTENT_HIDDEN_BY_USER, n.content)
        assertEquals("Chat", n.appName)
        assertEquals(1_000L, n.whenUnixMs)
        assertEquals(NotificationCategory.NOTIFICATION_CATEGORY_MESSAGE, n.category)
        assertEquals("", n.title)
        assertEquals("", n.text)
        assertEquals("", n.subText)
        assertEquals(0, n.messagesCount)
        assertEquals(0, n.actionsCount)
        assertFalse(n.hasLargeIcon())
        assertFalse(n.hasProgress())
        assertTrue(n.hasAppIcon())
        assertTrue(m.actions.isEmpty())
    }

    @Test
    fun `image hashes are collected for pinning`() {
        val m = mapper.map(base.copy(appIcon = png(1), largeIcon = png(2), picture = png(3)))
        assertEquals(3, m.imageHashes.size)
        assertEquals(m.imageHashes, setOf(m.notification.appIcon.sha256, m.notification.largeIcon.sha256, m.notification.picture.sha256))
    }
}
