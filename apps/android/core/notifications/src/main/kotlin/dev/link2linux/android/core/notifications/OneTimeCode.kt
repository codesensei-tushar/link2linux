package dev.link2linux.android.core.notifications

import java.text.Normalizer

/**
 * Spots notifications that look like they hold a one-time code, such as a
 * sign-in, payment or verification code, so the phone can keep them to
 * itself. Android 15 and later withhold most of these from this app on their
 * own (see [RedactionDetector]); this covers the versions before, and the
 * codes Android does not recognise.
 *
 * It fires only when a code-shaped token (`482913`, `482-913`, `482 913`,
 * `G-482913`, `K7Q2XM`) sits near a keyword such as "code", "OTP" or
 * "verification", in English or one of a dozen other languages. A keyword or
 * a token alone is not enough. Numbers that read as something else are not
 * tokens: phone numbers, dates, times, amounts, percentages, counts, years
 * away from a keyword, and numbers labelled as an order, a tracking number, a
 * card and the like. It leans towards catching codes, since a false positive
 * only hides one notification's text on the computer.
 */
class OneTimeCodeDetector {
    private val keywords = STRONG.map { Keyword.of(it, strong = true) } + WEAK.map { Keyword.of(it, strong = false) }
    private val labels = LABELS.map(::foldLower).toSet()
    private val fillers = FILLERS.map(::foldLower).toSet()
    private val qualifiers = QUALIFIERS.map(::foldLower).toSet()
    private val measures = MEASURES.map(::foldLower).toSet()
    private val currencies = CURRENCIES.map(::foldLower).toSet()

    /** True when any text of [s] that could reach the desktop looks like it holds a code, action labels included. */
    fun detect(s: NotificationSnapshot): Boolean = containsCode(
        buildList {
            add(s.title)
            add(s.bigTitle)
            add(s.text)
            add(s.bigText)
            add(s.subText)
            s.conversation?.let { c ->
                add(c.title)
                c.messages.forEach { add(it.sender + ": " + it.text) }
            }
            s.actions.forEach { a ->
                add(a.label)
                a.reply?.choices?.let(::addAll)
            }
            addAll(s.suggestedReplies)
        },
    )

    /** True when [fields], the texts of one notification, look like they hold a code. */
    fun containsCode(fields: List<String>): Boolean {
        val shown = fields.filter { it.isNotBlank() }.joinToString(FIELD_BREAK.toString()) { fold(it.take(MAX_FIELD)) }
        val lower = shown.lowerChars()
        return candidates(shown, lower).any { keywordNear(lower, it) }
    }

    private class Word(val start: Int, val end: Int, val digits: Int) {
        val length get() = end - start
        val numeric get() = digits == length
    }

    private class Candidate(val start: Int, val end: Int, val yearLike: Boolean = false)

    private class Keyword(val text: String, val strong: Boolean, private val prefix: Boolean) {
        // Latin keywords must stand as words; other scripts may not put spaces between words.
        private val latin = text.all { it.code < 128 }

        fun matchesAt(s: String, start: Int): Boolean =
            !latin || (!s.isWordChar(start - 1) && (prefix || !s.isWordChar(start + text.length)))

        companion object {
            fun of(spec: String, strong: Boolean) = Keyword(foldLower(spec.removeSuffix("*")), strong, spec.endsWith("*"))
        }
    }

    // Tokens

    private fun candidates(shown: String, s: String): List<Candidate> {
        val words = words(s)
        val found = ArrayList<Candidate>()
        var k = 0
        while (k < words.size) {
            val w = words[k]
            val next = words.getOrNull(k + 1)?.takeIf { it.start == w.end + 1 && s[w.end] == '-' }
            when {
                // G-482913: a short letter prefix, then the digits, which are the code.
                next != null && w.digits == 0 && w.length <= 3 && next.numeric && next.length in 4..8 -> {
                    found += Candidate(next.start, next.end)
                    k += 2
                }
                w.numeric -> k = numericRun(s, words, k, found)
                // 7HQ-4WP: two short upper-case halves with letters and digits.
                next != null && w.length in 3..5 && next.length in 3..5 && !next.numeric && w.digits + next.digits > 0 &&
                    shown.substring(w.start, next.end).none { it.isLowerCase() } -> {
                    found += Candidate(w.start, next.end)
                    k += 2
                }
                else -> {
                    if (isMixedCode(shown, s, w)) found += Candidate(w.start, w.end)
                    k++
                }
            }
        }
        return found.filterNot { labelled(s, it.start) || measured(s, it) }
    }

    /** Reads a number joined across single separators from words[k] on, adds its code-shaped parts and returns the index after it. */
    private fun numericRun(s: String, words: List<Word>, k: Int, found: MutableList<Candidate>): Int {
        var last = k
        val separators = StringBuilder()
        while (true) {
            val a = words[last]
            val b = words.getOrNull(last + 1) ?: break
            val separator = s[a.end]
            if (!b.numeric || b.start != a.end + 1 || separator !in JOINERS) break
            // A comma or a full stop joins only what reads as one number, 1,250 or 12.50, not "482913，5分钟".
            if ((separator == ',' || separator == '.') && b.length !in 2..3) break
            separators.append(separator)
            last++
        }
        val groups = words.subList(k, last + 1)
        val first = groups.first()
        val total = groups.sumOf { it.length }
        when {
            // Times, dates, amounts, decimals and versions.
            separators.any { it in ":/.,'" } -> Unit
            // An international number or an area code.
            precededBy(s, first.start, "+(") -> Unit
            groups.size == 1 -> if (first.length in 4..10) found += Candidate(first.start, first.end, isYear(s, first))
            // Spaces and hyphens mixed, or three groups or more with nine digits or more, are a phone number.
            ('-' in separators && ' ' in separators) || (groups.size >= 3 && total >= 9) -> Unit
            // 482-913, 4829-1375, 482 913, 48 29 13. Hyphenated pairs of digits are dates: 26-10-26.
            groups.all { it.length == first.length } && total in 6..10 &&
                first.length in (if ('-' in separators) 3..4 else 2..4) ->
                found += Candidate(first.start, groups.last().end)
            // Other hyphenated numbers are dates, phone numbers or references.
            '-' in separators -> Unit
            // Separate numbers that happen to stand next to each other.
            else -> groups.filter { it.length in 4..10 }.forEach { found += Candidate(it.start, it.end, isYear(s, it)) }
        }
        return last + 1
    }

    private fun isMixedCode(shown: String, s: String, w: Word): Boolean {
        if (w.length !in 4..10 || w.digits == 0 || w.numeric || s.getOrNull(w.end) == '@') return false
        val lower = s.substring(w.start, w.end)
        // 2nd, 1080p, 30mins; iphone15, covid19; promo codes such as save20 and welcome100.
        if (ORDINAL_OR_MEASURE.matches(lower) || WORD_AND_NUMBER.matches(lower)) return false
        if (keywords.any { it.text == lower }) return false
        val upper = shown.substring(w.start, w.end).none { it.isLowerCase() }
        return w.digits >= 2 || upper
    }

    private fun isYear(s: String, w: Word) = w.length == 4 && s.substring(w.start, w.end).toInt() in 1900..2099

    /** Labelled as something other than a code: "order #4839201", "card ending in 1234". */
    private fun labelled(s: String, start: Int): Boolean {
        var at = start
        repeat(MAX_FILLERS + 1) {
            val (word, wordStart) = wordBefore(s, at) ?: return false
            if (word !in fillers) return isLabel(word)
            at = wordStart
        }
        return false
    }

    private fun isLabel(word: String): Boolean = labels.any { label ->
        when {
            word == label -> true
            label.all { it.code < 128 } -> label.length >= 5 && word.startsWith(label)
            // Other scripts may inflect a word or run several together: "заказа", "您的订单号".
            else -> word.endsWith(label) || word.dropLast(1).endsWith(label)
        }
    }

    /** An amount, a percentage or a count: "₹4999", "Rs.4999", "1250 rupees", "4500 steps". */
    private fun measured(s: String, c: Candidate): Boolean {
        var before = c.start - 1
        while (before >= 0 && (s[before] == ' ' || s[before] == '.')) before--
        if (before >= 0 && s[before].isCurrencySymbol()) return true
        if (wordBefore(s, c.start)?.first in currencies) return true
        var after = c.end
        while (after < s.length && s[after] == ' ') after++
        val sign = s.getOrNull(after)
        if (sign == '%' || sign?.isCurrencySymbol() == true) return true
        val (word, _) = wordAfter(s, c.end) ?: return false
        return word in measures || word in currencies || measures.any { it.first().code >= 128 && word.startsWith(it) }
    }

    // Keywords

    private fun keywordNear(s: String, c: Candidate): Boolean {
        // Searched only around the token, so a long email costs no more than a short message.
        val from = (c.start - STRONG_REACH).coerceAtLeast(0)
        val window = s.substring(from, (c.end + STRONG_REACH).coerceAtMost(s.length))
        for (k in keywords) {
            var found = window.indexOf(k.text)
            while (found >= 0) {
                val start = from + found
                val end = start + k.text.length
                val gap = when {
                    end <= c.start -> s.substring(end, c.start)
                    start >= c.end -> s.substring(c.end, start)
                    else -> null
                }
                if (gap != null && k.matchesAt(s, start) && !qualified(s, start, end)) {
                    val near = when {
                        // "Happy 2026" is not a code, "code: 2026" is.
                        c.yearLike -> FIELD_BREAK !in gap && gap.length <= YEAR_REACH
                        k.strong -> gap.length <= STRONG_REACH
                        else -> gap.length <= WEAK_REACH
                    }
                    if (near) return true
                }
                found = window.indexOf(k.text, found + 1)
            }
        }
        return false
    }

    /** "promo code", "error code", "código de descuento": a code that is no secret. */
    private fun qualified(s: String, start: Int, end: Int): Boolean {
        if (wordBefore(s, start)?.first in qualifiers) return true
        val (word, wordEnd) = wordAfter(s, end) ?: return false
        val after = if (word in LINKS) wordAfter(s, wordEnd)?.first else word
        return after in qualifiers
    }

    /** The word of letters (and `/`, for "a/c") before [at], skipping punctuation, and where it starts. */
    private fun wordBefore(s: String, at: Int): Pair<String, Int>? {
        var i = at - 1
        while (i >= 0 && !s.isWordChar(i) && s[i] != '/') {
            if (s[i] == FIELD_BREAK) return null
            i--
        }
        val end = i + 1
        while (i >= 0 && (s[i].isLetter() || s[i] == '/')) i--
        return if (i + 1 < end) s.substring(i + 1, end) to i + 1 else null
    }

    /** The word of letters right after [at], past spaces, and where it ends. */
    private fun wordAfter(s: String, at: Int): Pair<String, Int>? {
        var i = at
        while (i < s.length && s[i] == ' ') i++
        val start = i
        while (i < s.length && s[i].isLetter()) i++
        return if (i > start) s.substring(start, i) to i else null
    }

    private fun precededBy(s: String, start: Int, signs: String): Boolean {
        val before = s.getOrNull(start - 1) ?: return false
        return before in signs || (before == ' ' && s.getOrNull(start - 2)?.let { it in signs } == true)
    }

    private companion object {
        /** Joins the fields. A title "Your code" and a text "4821" read as one, but a label never reaches across. */
        const val FIELD_BREAK = '\u001f'

        /** At least what the wire carries of a field, so every character that could reach the desktop is read. */
        const val MAX_FIELD = Limits.LONG_TEXT

        const val STRONG_REACH = 120
        const val WEAK_REACH = 24
        const val YEAR_REACH = 12
        const val MAX_FILLERS = 3
        const val JOINERS = " -.:/,'"

        val ORDINAL_OR_MEASURE = Regex("\\d+(st|nd|rd|th|am|pm|[kmgt]b|px|ms|s|m|h|d|km|kg|g|hz|fps|mph|p|k|x|mins?|hrs?|secs?|yrs?)")
        val WORD_AND_NUMBER = Regex("[a-z]{4,}\\d+|[a-z]{3}\\d{1,3}")

        /** Small words between a keyword and a qualifier after it: "código de descuento". */
        val LINKS = setOf("de", "di", "da", "du", "do", "del", "della")

        /**
         * Accents dropped, every script's digits as ASCII digits and full-width
         * forms folded, so "Código: ４８２９１３" reads as "Codigo: 482913". Keeps
         * the length of what it returns lower-cased with [lowerChars].
         */
        fun fold(text: String): String {
            val out = StringBuilder(text.length)
            for (c in Normalizer.normalize(text, Normalizer.Form.NFKD)) {
                when {
                    c in '̀'..'ͯ' -> Unit
                    c == '’' || c == '‘' -> out.append('\'')
                    // Turkish dotless i, so "KULLANIMLIK" and "kullanımlık" read the same.
                    c == 'ı' -> out.append('i')
                    Character.isDigit(c) -> out.append('0' + Character.digit(c, 10))
                    c == FIELD_BREAK || Character.isISOControl(c) -> out.append(' ')
                    else -> out.append(c)
                }
            }
            return Normalizer.normalize(out, Normalizer.Form.NFC)
        }

        fun foldLower(text: String) = fold(text).lowerChars()

        /** Lower case one char at a time, so indexes stay valid. */
        fun String.lowerChars() = buildString(length) { this@lowerChars.forEach { append(it.lowercaseChar()) } }

        fun String.isWordChar(i: Int): Boolean = getOrNull(i)?.isLetterOrDigit() == true

        fun Char.isCurrencySymbol() = Character.getType(this) == Character.CURRENCY_SYMBOL.toInt()

        fun words(s: String): List<Word> {
            val words = ArrayList<Word>()
            var i = 0
            while (i < s.length) {
                if (!s[i].isAsciiAlnum()) {
                    i++
                    continue
                }
                val start = i
                var digits = 0
                while (i < s.length && s[i].isAsciiAlnum()) {
                    if (s[i] in '0'..'9') digits++
                    i++
                }
                words += Word(start, i, digits)
            }
            return words
        }

        fun Char.isAsciiAlnum() = this in 'a'..'z' || this in '0'..'9'

        /** Enough anywhere near a token. A trailing `*` matches any ending. */
        val STRONG = listOf(
            // English
            "otp", "one-time", "onetime", "verification", "passcode", "2fa", "two-factor", "two factor", "2-step",
            "2-factor", "2sv", "security code", "login code", "log-in code", "log in code", "sign-in code",
            "sign in code", "signin code", "authentication", "auth code", "access code", "confirmation code",
            "do not share", "don't share", "never share", "do not disclose", "do not give", "don't give",
            // Spanish
            "código de verificación", "verificación", "código de seguridad", "código de acceso",
            "código de confirmación", "un solo uso", "no compartas", "no lo compartas", "no comparta", "no compartir",
            // French
            "code de vérification", "vérification", "code de sécurité", "code d'accès", "code de confirmation",
            "usage unique", "ne le partagez", "ne partagez", "ne communiquez",
            // German
            "bestätigungscode", "verifizierungscode", "sicherheitscode", "anmeldecode", "zugangscode", "einmalcode",
            "einmalpasswort", "einmal-passwort", "einmalkennwort", "aktivierungscode", "freigabecode",
            "nicht weiter", "nicht teilen", "mtan", "smstan", "sms-tan", "pushtan",
            // Portuguese
            "código de verificação", "verificação", "código de segurança", "não compartilhe", "não partilhe",
            "não compartilhar", "uso único",
            // Italian
            "codice di verifica", "codice di sicurezza", "codice di accesso", "non condividere", "non condividerlo",
            "monouso",
            // Hindi, in Devanagari and romanized
            "ओटीपी", "सत्यापन", "वन टाइम", "साझा न करें", "शेयर न करें", "share na kare", "share na karein",
            "share mat kare", "share mat karo", "kisi ke saath share",
            // Russian
            "код подтверждения", "код проверки", "проверочный код", "одноразов", "никому не сообщайте",
            "не сообщайте", "не передавайте", "никому не говорите", "код для входа",
            // Chinese
            "验证码", "校验码", "动态码", "动态密码", "安全码", "确认码", "登录码", "一次性", "请勿泄露", "切勿泄露",
            "请勿告知", "驗證碼", "認證碼", "動態密碼",
            // Japanese
            "認証コード", "確認コード", "認証番号", "確認番号", "ワンタイム", "パスコード", "セキュリティコード",
            "ログインコード", "検証コード", "他人に教え", "誰にも教え", "共有しないで",
            // Korean
            "인증번호", "인증 번호", "인증코드", "인증 코드", "확인코드", "확인 코드", "보안코드", "보안 코드", "일회용",
            "타인에게 노출", "공유하지 마", "알려주지 마",
            // Arabic
            "رمز التحقق", "التحقق", "رمز التفعيل", "لمرة واحدة", "لا تشارك", "رمز الدخول", "رمز الأمان",
            // Indonesian
            "kode verifikasi", "verifikasi", "kode otp", "jangan berikan", "jangan bagikan", "jangan memberikan",
            "rahasiakan", "sekali pakai", "kode keamanan", "kode akses",
            // Turkish
            "doğrulama", "tek kullanımlık", "paylaşmayın", "paylaşmayınız", "güvenlik kodu", "onay kodu",
            "giriş kodu",
        )

        /** Count only right next to a token: "Your Uber code: 1234", "Copy 482913". */
        val WEAK = listOf(
            "code*", "verif*", "pin", "password*", "passwd", "confirm*", "one time", "login", "log in", "sign in",
            "sign-in", "signin", "token", "copy",
            "código*", "clave", "contraseña", "confirma*", "copiar",
            "mot de passe", "copier",
            "tan", "passwort", "kennwort", "bestätig*", "anmeld*", "kopieren",
            "senha",
            "codice", "conferma*", "copia",
            "कोड", "पासवर्ड", "पिन",
            "код", "пароль", "подтвержд", "скопировать", "копировать",
            "密码", "验证", "驗證", "代码", "复制",
            "コード", "暗証番号", "パスワード", "コピー",
            "코드", "비밀번호", "인증", "복사",
            "رمز", "كود", "كلمة المرور", "كلمة السر", "نسخ",
            "kode", "kata sandi", "sandi", "salin",
            "kod*", "şifre*", "kopyala",
        )

        /** Words before a number that make it an identifier. Latin ones of five letters or more match any ending. */
        val LABELS = listOf(
            "order", "tracking", "track", "shipment", "parcel", "package", "invoice", "receipt", "ticket", "flight",
            "train", "seat", "gate", "room", "table", "account", "acct", "a/c", "card", "ref", "reference", "booking",
            "reservation", "pnr", "case", "issue", "pr", "build", "version", "model", "serial", "imei", "id",
            "extension", "ext", "apt", "unit", "suite", "call", "phone", "mobile", "tel", "contact", "helpline",
            "dial", "fax", "promo", "coupon", "voucher", "referral", "postal", "zip", "postcode",
            "pedido", "rastreo", "seguimiento", "paquete", "factura", "cuenta", "tarjeta", "reserva",
            "commande", "suivi", "colis", "facture", "compte", "carte",
            "bestell", "sendung", "rechnung", "konto", "karte",
            "encomenda", "fatura", "conta", "cartao",
            "ordine", "spedizione", "fattura", "carta",
            "pesanan", "resi", "rekening", "kartu",
            "siparis", "kargo",
            "заказ", "посылк", "счет", "карт",
            "订单", "单号", "快递", "注文", "伝票", "주문", "송장", "طلب", "شحنة",
        )

        /** Words that may stand between a label and its number: "order no. 4839201", "card ending in 1234". */
        val FILLERS = listOf("no", "n", "nr", "num", "number", "numero", "nummer", "is", "ending", "ends", "in", "with", "at", "on")

        /** Words next to a keyword that make it a code that is no secret: "promo code", "código promocional". */
        val QUALIFIERS = listOf(
            "promo", "promocional", "promozionale", "promotional", "promotion", "coupon", "cupón", "discount",
            "descuento", "sconto", "réduction", "rabatt", "voucher", "gift", "regalo", "referral", "invite",
            "invitation", "postal", "zip", "post", "area", "country", "país", "error", "erreur", "fehler", "errore",
            "status", "exit", "product", "producto", "tracking", "qr", "bar", "barras", "source", "colour", "color",
            "dress", "review", "booking", "reserva", "reservation", "reservierung", "prenotazione", "kupon",
            "indirim", "promosyon",
        )

        /** Words after a number that make it a count or a measure. Non-Latin ones match any ending. */
        val MEASURES = listOf(
            "min", "mins", "minute", "minutes", "hr", "hrs", "hour", "hours", "sec", "secs", "second", "seconds",
            "day", "days", "week", "weeks", "month", "months", "year", "years", "yr", "yrs", "km", "kms", "kg",
            "kgs", "mb", "gb", "kb", "tb", "step", "steps", "point", "points", "pts", "pt", "coin", "coins",
            "follower", "followers", "like", "likes", "view", "views", "mile", "miles", "meter", "meters", "metre",
            "metres", "item", "items", "people", "person", "persons", "unit", "units", "times", "new", "more",
            "unread", "message", "messages", "email", "emails", "notifications", "comments", "replies", "members",
            "users", "downloads",
            "分", "円", "元", "원", "руб",
        )

        val CURRENCIES = listOf(
            "rs", "inr", "usd", "eur", "gbp", "rp", "idr", "rm", "aed", "sar", "jpy", "cny", "rmb", "krw", "rub",
            "mxn", "brl", "tl", "chf", "cad", "aud", "sgd", "hkd", "rupees", "rupee", "dollars", "dollar", "euro",
            "euros", "pounds", "yen", "lira", "reais", "pesos", "rupiah", "ريال", "درهم",
        )
    }
}
