package com.salihbayrak.wabulk

import android.content.Context

/**
 * Uygulamanın ortak deposu.
 * - Alıcı listesi ve sonuçlar bellekte tutulur (uygulama kapanınca silinir).
 * - Mesaj metni ve bekleme süreleri SharedPreferences'ta saklanır.
 */
object Store {

    // Ad + uluslararası telefon (sadece rakam, örn. 905321234567)
    data class Recipient(val name: String, val phone: String)

    // Gönderim sonucu: durum = gönderildi / gönderilemedi / başarısız
    data class Result(val name: String, val phone: String, val status: String, val detail: String)

    val recipients: MutableList<Recipient> = mutableListOf()
    val results: MutableList<Result> = mutableListOf()

    private const val PREFS = "wabulk"
    private const val KEY_MESSAGE = "message"
    private const val KEY_MIN = "min_wait"
    private const val KEY_MAX = "max_wait"
    private const val KEY_COUNTRY = "country_code"

    private fun prefs(ctx: Context) = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    // --- Mesaj metni ---
    fun getMessage(ctx: Context): String = prefs(ctx).getString(KEY_MESSAGE, "") ?: ""

    fun setMessage(ctx: Context, value: String) {
        prefs(ctx).edit().putString(KEY_MESSAGE, value).apply()
    }

    // --- En az bekleme (saniye), varsayılan 4 ---
    fun getMinWait(ctx: Context): Int = prefs(ctx).getInt(KEY_MIN, 4)

    fun setMinWait(ctx: Context, value: Int) {
        prefs(ctx).edit().putInt(KEY_MIN, value).apply()
    }

    // --- En çok bekleme (saniye), varsayılan 9 ---
    fun getMaxWait(ctx: Context): Int = prefs(ctx).getInt(KEY_MAX, 9)

    fun setMaxWait(ctx: Context, value: Int) {
        prefs(ctx).edit().putInt(KEY_MAX, value).apply()
    }

    // --- Ülke kodu, varsayılan 90 ---
    fun getCountryCode(ctx: Context): String = prefs(ctx).getString(KEY_COUNTRY, "90") ?: "90"

    fun setCountryCode(ctx: Context, value: String) {
        prefs(ctx).edit().putString(KEY_COUNTRY, value).apply()
    }

    /** Aynı numara zaten listede varsa tekrar eklenmez. */
    fun addRecipients(list: List<Recipient>) {
        for (r in list) {
            if (recipients.none { it.phone == r.phone }) recipients.add(r)
        }
    }

    /**
     * Rehberden gelen ham numarayı uluslararası biçime çevirir:
     * rakam dışını at, baştaki 00'ı at, ülke koduyla başlıyorsa dokunma,
     * baştaki 0'ı at ve ülke kodunu ekle.
     */
    fun normalizePhone(raw: String, countryCode: String): String {
        var digits = raw.filter { it.isDigit() }
        if (digits.startsWith("00")) digits = digits.substring(2)
        val cc = countryCode.filter { it.isDigit() }
        if (digits.isEmpty()) return ""
        if (cc.isNotEmpty() && digits.startsWith(cc)) return digits
        if (digits.startsWith("0")) digits = digits.substring(1)
        if (digits.isEmpty()) return ""
        return cc + digits
    }

    /** Mesajdaki "/isim" yerine adın ilk kelimesini koyar. */
    fun personalize(message: String, name: String): String {
        val first = name.trim().split(" ").firstOrNull { it.isNotBlank() } ?: ""
        return message.replace("/isim", first)
    }
}
