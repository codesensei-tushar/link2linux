package dev.link2linux.android.core.notifications

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class OneTimeCodeDetectorTest {
    private val detector = OneTimeCodeDetector()

    /** Checks every case and names all the wrong ones at once. */
    private fun assertEach(cases: List<String>, code: Boolean) {
        val wrong = cases.filter { detector.containsCode(listOf(it)) != code }
        assertTrue("expected ${if (code) "a code" else "no code"} in:\n" + wrong.joinToString("\n"), wrong.isEmpty())
    }

    @Test
    fun `codes as apps and banks send them are caught`() = assertEach(
        listOf(
            "G-482913 is your Google verification code.",
            "Your WhatsApp code: 482-913\nYou can also tap on this link to verify your phone: v.whatsapp.com/482913",
            "Your Uber code: 4821. Never share this code. Reply STOP ALL to unsubscribe.",
            "482913 is your Amazon OTP. Do not share it with anyone.",
            "Login code: 48291. Do not give this code to anyone, even if they say they are from Telegram!",
            "Dear Customer, 482913 is the OTP for your transaction of INR 1,250.00 at AMAZON on your card " +
                "ending 1234. Valid for 5 mins. Do not share OTP with anyone.",
            "OTP for txn of Rs.1,250.00 at AMAZON on card XX1234 is 482913. Valid till 10:45. Do not share.",
            "Your Instagram code is 482 913. Don't share it.",
            "Use 482913 to verify your Instagram account.",
            "PayPal: 482913 is your security code. Don't share your code.",
            "Your one-time code is 48291375. Don’t share it. We'll never call to ask for it.",
            "Your Apple Account code is: 482913. Don't share it with anyone.",
            "Your Steam Guard code: FRV9W",
            "Your verification code is K7Q2XM",
            "Your one time password is 482913. It expires in 10 minutes.",
            "Enter 4821 to sign in to your account",
            "Your PIN is 4821",
            "Your LinkedIn verification code is 482913.",
            "Your security code is 7HQ-4WP",
            "Your code is 48 29 13",
            "Share OTP 4821 with the delivery agent to receive your order #4839201",
            "482913 is your Example code.\n\n@example.com #482913",
            "Your temporary password is Xk82mq",
            "Your code is 1987",
            "Your 2FA code: 482913",
            "Code: 482913",
        ),
        code = true,
    )

    @Test
    fun `codes in other languages and scripts are caught`() = assertEach(
        listOf(
            "Tu código de WhatsApp es 482-913. No compartas este código con nadie.",
            "Tu código de verificación es 482913",
            "Votre code de vérification est 482913. Ne le partagez avec personne.",
            "Votre code WhatsApp : 482-913",
            "Ihr Bestätigungscode lautet 482913. Geben Sie ihn nicht weiter.",
            "Ihre TAN: 482913",
            "Seu código de verificação é 482913. Não compartilhe.",
            "Use 482913 como código de segurança",
            "Il tuo codice di verifica è 482913",
            "आपका ओटीपी 482913 है। इसे किसी के साथ साझा न करें।",
            "Aapka OTP 482913 hai. Kisi ke saath share na karein.",
            "आपका सत्यापन कोड ४८२९१३ है",
            "Ваш код подтверждения: 482913. Никому не сообщайте его.",
            "Код для входа в Telegram: 48291",
            "您的验证码是482913，5分钟内有效，请勿泄露。",
            "您的驗證碼為 482913",
            "認証コード：４８２９１３",
            "あなたの確認コードは 482913 です",
            "[Web발신] 인증번호 [482913]를 입력해주세요.",
            "رمز التحقق الخاص بك هو 482913",
            "رمز التحقق: ٤٨٢٩١٣",
            "Kode verifikasi Anda 482913. Jangan berikan kode ini kepada siapa pun.",
            "Doğrulama kodunuz: 482913",
            "TEK KULLANIMLIK ŞİFRENİZ 482913",
        ),
        code = true,
    )

    @Test
    fun `everyday notifications are not codes`() = assertEach(
        listOf(
            "Your order #4839201 has shipped",
            "Meeting at 10:30",
            "Call me at +1 415 555 0134",
            "Call me at 415-555-0134 to confirm",
            "Call (415) 555-0134 to verify your booking",
            "You paid ₹1,250.00",
            "Payment of USD 1250 received",
            "Happy 2026!",
            "Order 4839201 confirmed",
            "Your booking 4839201 is confirmed for 2 guests",
            "Track your parcel: 4839201734",
            "Rs 4,999 debited from A/c XX1234 on 26-10-26. Not you? Call 18002094324",
            "Confirm your appointment on 12/11 at 14:30",
            "Your verification code expires in 10 minutes",
            "You have 4821 unread messages",
            "lol I got 4821 points in that game",
            "You walked 10000 steps today",
            "Battery at 15%",
            "Downloading holiday-photos.zip: 62% of 1.2 GB",
            "Flight UA 1234 departs at 10:45 from gate B12",
            "Version 4.12.1 is available",
            "Use code SAVE20 for 20% off your next order",
            "Promo code WELCOME100 gets you 100 off",
            "Error code 4031: try again later",
            "Your 2-step verification settings were changed",
            "New sign-in to your account from Chrome on Linux",
            "Ann: are we still on for 2026-10-30?",
            "My new number is 98765 43210, save it",
            "The meeting ID is 482 913 4421",
            "Room 4021 is ready, see you at 7",
            "Your ride is arriving. Plate KA01AB1234",
            "Reminder: dentist at 1430 tomorrow",
            "Earn 5000 coins with code BONUS",
            "Can you send the report by Friday? It's about 4500 words",
        ),
        code = false,
    )

    @Test
    fun `a keyword alone or a token alone is not enough`() {
        assertFalse(detector.containsCode(listOf("Your verification code is ready")))
        assertFalse(detector.containsCode(listOf("Never share your OTP with anyone")))
        assertFalse(detector.containsCode(listOf("482913")))
        assertFalse(detector.containsCode(listOf("Bank", "Your account was updated", "482913")))
    }

    @Test
    fun `a keyword in the title vouches for a code in the text`() {
        assertTrue(detector.containsCode(listOf("Verification code", "482913")))
        assertTrue(detector.containsCode(listOf("Your code", "4821")))
    }

    @Test
    fun `a year needs a keyword right next to it in the same field`() {
        assertTrue(detector.containsCode(listOf("Your code is 2026")))
        assertFalse(detector.containsCode(listOf("Your verification is done, see you in 2026")))
        assertFalse(detector.containsCode(listOf("Code", "2026")))
    }

    private fun snapshot(
        text: String = "",
        actions: List<ActionSnapshot> = emptyList(),
        conversation: ConversationSnapshot? = null,
        suggested: List<String> = emptyList(),
    ) = NotificationSnapshot(
        key = "k",
        packageName = "org.example",
        title = "Messages",
        text = text,
        actions = actions,
        conversation = conversation,
        suggestedReplies = suggested,
    )

    private fun action(label: String, choices: List<String> = emptyList()) = ActionSnapshot(
        label = label,
        reply = if (choices.isEmpty()) null else ReplyInput(choices = choices),
        invoker = { _, _ -> ActionOutcome.SENT },
    )

    @Test
    fun `action labels, reply choices and suggestions are read`() {
        assertTrue(detector.detect(snapshot(text = "New message", actions = listOf(action("Copy 482913")))))
        assertTrue(detector.detect(snapshot(actions = listOf(action("Reply", choices = listOf("Code: 482913"))))))
        assertTrue(detector.detect(snapshot(suggested = listOf("Copy \"482913\""))))
        assertFalse(detector.detect(snapshot(text = "See you at 7", actions = listOf(action("Mark as read")))))
    }

    @Test
    fun `conversation messages are read`() {
        val chat = ConversationSnapshot(
            listOf(
                MessageSnapshot("Ann", "what's the door code?", 1, outgoing = false),
                MessageSnapshot("Bob", "4821", 2, outgoing = false),
            ),
        )
        assertTrue(detector.detect(snapshot(conversation = chat)))
        val plain = ConversationSnapshot(listOf(MessageSnapshot("Ann", "lunch at 1?", 1, outgoing = false)))
        assertFalse(detector.detect(snapshot(conversation = plain)))
    }
}
