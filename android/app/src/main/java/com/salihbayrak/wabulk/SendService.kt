package com.salihbayrak.wabulk

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Build
import android.os.IBinder
import java.net.URLEncoder
import java.util.Random

/**
 * Toplu gönderimi arka planda (ön plan servisi olarak) yürütür.
 * Her kişi için wa.me bağlantısıyla WhatsApp sohbetini açar, erişilebilirlik
 * servisi ile gönder düğmesine basar ve geri döner.
 */
class SendService : Service() {

    companion object {
        const val ACTION_START = "com.salihbayrak.wabulk.ACTION_START"
        const val ACTION_STOP = "com.salihbayrak.wabulk.ACTION_STOP"
        const val ACTION_UPDATE = "com.salihbayrak.wabulk.UPDATE"
        const val EXTRA_TEXT = "text"
        const val EXTRA_RUNNING = "running"

        /** MainActivity "Durdur" dediğinde true olur. */
        @Volatile
        var stop: Boolean = false
    }

    private val kanalId = "send"
    private val bildirimId = 1
    private var bildirimYoneticisi: NotificationManager? = null
    private var isParcacigi: Thread? = null

    override fun onCreate() {
        super.onCreate()
        kanalOlustur()
        // Android, ön plan servisinin 5 saniye içinde bildirim göstermesini ister.
        onePlanaAl("Hazırlanıyor...")
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stop = true
                bildirimGuncelle("Durduruluyor...")
                // Döngü çalışmıyorsa servis hemen kapanır.
                if (isParcacigi?.isAlive != true) kapat()
            }
            else -> {
                stop = false
                donguBaslat()
            }
        }
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        stop = true
        super.onDestroy()
    }

    // ---------------------------------------------------------------- döngü

    private fun donguBaslat() {
        if (isParcacigi?.isAlive == true) return
        val t = Thread { dongu() }
        isParcacigi = t
        t.start()
    }

    private fun dongu() {
        // Liste kopyalanır: kullanıcı arayüzden değiştirirse çökmesin.
        val liste = ArrayList(Store.recipients)
        val toplam = liste.size

        if (toplam == 0) {
            broadcast("Liste boş, gönderim yapılmadı.", false)
            kapat()
            return
        }

        val mesaj = Store.getMessage(this)
        var minBekleme = Store.getMinWait(this)
        var maxBekleme = Store.getMaxWait(this)
        if (maxBekleme < minBekleme) maxBekleme = minBekleme
        val rnd = Random()

        val baslangic = System.currentTimeMillis()
        var gonderilen = 0
        var gonderilemeyen = 0

        for (i in 0 until toplam) {
            if (stop) break

            val r = liste[i]
            val sira = i + 1
            val metin = Store.personalize(mesaj, r.name)
            var durum = "gönderildi"
            var detay = ""

            bildirimGuncelle("$sira/$toplam • ${r.name}")

            // a) WhatsApp sohbetini aç
            if (!sohbetAc(r.phone, metin)) {
                durum = "başarısız"
                detay = "WhatsApp açılamadı"
                gonderilemeyen++
            } else {
                // b) sohbet açılsın, sonra gönder düğmesine bas
                bekle(2500)
                val tiklandi = try {
                    WaAccessibilityService.instance?.clickSend(6000) ?: false
                } catch (e: Exception) {
                    false
                }
                if (tiklandi) {
                    gonderilen++
                } else {
                    durum = "gönderilemedi"
                    detay = "numara yok olabilir ya da WhatsApp yavaş"
                    gonderilemeyen++
                }
                // c) gönderim otursun, sonra sohbetten listeye dön
                //    (hemen geri basmak gönderimi kesebiliyor)
                if (tiklandi) bekle(700)
                try {
                    WaAccessibilityService.instance?.goBack()
                } catch (e: Exception) {
                    // yok sayılır
                }
            }

            val gecen = (System.currentTimeMillis() - baslangic) / 1000
            Store.results.add(Store.Result(r.name, r.phone, durum, detay))

            val satir = "$sira/$toplam • ${r.name} • $durum" +
                (if (detay.isEmpty()) "" else " ($detay)") +
                " • geçen ${sureMetni(gecen)}"
            bildirimGuncelle("$sira/$toplam • geçen ${sureMetni(gecen)}")
            broadcast(satir, true)

            if (stop || sira >= toplam) break

            // d) her 25 mesajda uzun mola, yoksa rastgele bekleme
            if (sira % 25 == 0) {
                bildirimGuncelle("$sira/$toplam • uzun mola (60 sn)")
                if (!bekle(60_000)) break
            } else {
                val saniye = minBekleme + rnd.nextInt(maxBekleme - minBekleme + 1)
                if (!bekle(saniye * 1000L)) break
            }
        }

        val toplamSure = (System.currentTimeMillis() - baslangic) / 1000
        val ozet = if (stop) {
            "Durduruldu • $gonderilen gönderildi, $gonderilemeyen gönderilemedi • ${sureMetni(toplamSure)}"
        } else {
            "Bitti • $gonderilen gönderildi, $gonderilemeyen gönderilemedi • ${sureMetni(toplamSure)}"
        }
        broadcast(ozet, false)
        kapat()
    }

    /** Durdurma isteğini de kontrol ederek bekler. false = durduruldu. */
    private fun bekle(ms: Long): Boolean {
        var kalan = ms
        while (kalan > 0) {
            if (stop) return false
            val adim = if (kalan > 200) 200 else kalan
            try {
                Thread.sleep(adim)
            } catch (e: InterruptedException) {
                return false
            }
            kalan -= adim
        }
        return !stop
    }

    private fun sureMetni(saniye: Long): String {
        val dk = saniye / 60
        val sn = saniye % 60
        return if (dk > 0) "$dk dk $sn sn" else "$sn sn"
    }

    // ------------------------------------------------------------- whatsapp

    /** wa.me bağlantısı ile sohbeti açar; sırasıyla normal ve Business paketini dener. */
    private fun sohbetAc(phone: String, mesaj: String): Boolean {
        val url = "https://wa.me/$phone?text=" + URLEncoder.encode(mesaj, "UTF-8")
        val niyet = Intent(Intent.ACTION_VIEW, Uri.parse(url))
        niyet.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

        niyet.setPackage("com.whatsapp")
        try {
            startActivity(niyet)
            return true
        } catch (e: ActivityNotFoundException) {
            // WhatsApp yok ya da kullanılamıyor: Business sürümünü dene.
        } catch (e: Exception) {
            return false
        }

        niyet.setPackage("com.whatsapp.w4b")
        return try {
            startActivity(niyet)
            true
        } catch (e: Exception) {
            false
        }
    }

    // ------------------------------------------------------------ bildirim

    private fun kanalOlustur() {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        bildirimYoneticisi = nm
        val kanal = NotificationChannel(
            kanalId,
            getString(R.string.notif_channel),
            NotificationManager.IMPORTANCE_LOW
        )
        kanal.setShowBadge(false)
        nm.createNotificationChannel(kanal)
    }

    private fun bildirim(metin: String): Notification {
        val acilis = Intent(this, MainActivity::class.java)
        val pi = PendingIntent.getActivity(this, 0, acilis, PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, kanalId)
            .setSmallIcon(R.drawable.ic_launcher_foreground)
            .setContentTitle(getString(R.string.notif_title))
            .setContentText(metin)
            .setOngoing(true)
            .setContentIntent(pi)
            .build()
    }

    private fun onePlanaAl(metin: String) {
        val n = bildirim(metin)
        try {
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(bildirimId, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
            } else {
                startForeground(bildirimId, n)
            }
        } catch (e: Exception) {
            // Android 14'te tip/izin sorunu olursa en azından servis ayakta kalsın.
            try {
                startForeground(bildirimId, n)
            } catch (e2: Exception) {
                // yok sayılır
            }
        }
    }

    private fun bildirimGuncelle(metin: String) {
        try {
            bildirimYoneticisi?.notify(bildirimId, bildirim(metin))
        } catch (e: Exception) {
            // yok sayılır
        }
    }

    private fun broadcast(metin: String, calisiyor: Boolean) {
        try {
            val niyet = Intent(ACTION_UPDATE)
            niyet.setPackage(packageName)
            niyet.putExtra(EXTRA_TEXT, metin)
            niyet.putExtra(EXTRA_RUNNING, calisiyor)
            sendBroadcast(niyet)
        } catch (e: Exception) {
            // yok sayılır
        }
    }

    private fun kapat() {
        try {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } catch (e: Exception) {
            // yok sayılır
        }
        stopSelf()
    }
}
