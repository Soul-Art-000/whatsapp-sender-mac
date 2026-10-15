package com.salihbayrak.wabulk

import android.Manifest
import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.View
import android.view.ViewGroup
import android.widget.BaseAdapter
import android.widget.Button
import android.widget.EditText
import android.widget.ListView
import android.widget.TextView
import android.widget.Toast

/**
 * Ana ekran: kişi listesi, mesaj, bekleme süreleri, izin durumu,
 * gönder/durdur düğmeleri ve sonuç listesi.
 */
class MainActivity : Activity() {

    private lateinit var tvCount: TextView
    private lateinit var etMessage: EditText
    private lateinit var etMin: EditText
    private lateinit var etMax: EditText
    private lateinit var etCountry: EditText
    private lateinit var tvPerm: TextView
    private lateinit var btnContacts: Button
    private lateinit var btnNotif: Button
    private lateinit var btnAccess: Button
    private lateinit var btnOverlay: Button
    private lateinit var btnSend: Button
    private lateinit var btnStop: Button
    private lateinit var tvProgress: TextView
    private lateinit var resultAdapter: ResultAdapter

    private var receiverKayitli = false

    private val izinKoduKisi = 101
    private val izinKoduBildirim = 102

    // SendService'ten gelen ilerleme yayınlarını dinler.
    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            val metin = intent?.getStringExtra(SendService.EXTRA_TEXT) ?: return
            val calisiyor = intent.getBooleanExtra(SendService.EXTRA_RUNNING, false)
            tvProgress.text = metin
            if (!calisiyor) {
                duymeleriGuncelle(false)
                sonuclariTazele()
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)

        tvCount = findViewById(R.id.tvCount)
        etMessage = findViewById(R.id.etMessage)
        etMin = findViewById(R.id.etMin)
        etMax = findViewById(R.id.etMax)
        etCountry = findViewById(R.id.etCountry)
        tvPerm = findViewById(R.id.tvPerm)
        btnContacts = findViewById(R.id.btnContacts)
        btnNotif = findViewById(R.id.btnNotif)
        btnAccess = findViewById(R.id.btnAccess)
        btnOverlay = findViewById(R.id.btnOverlay)
        btnSend = findViewById(R.id.btnSend)
        btnStop = findViewById(R.id.btnStop)
        tvProgress = findViewById(R.id.tvProgress)

        resultAdapter = ResultAdapter()
        findViewById<ListView>(R.id.listResults).adapter = resultAdapter

        // Kayıtlı ayarları yükle
        etMessage.setText(Store.getMessage(this))
        etMin.setText(Store.getMinWait(this).toString())
        etMax.setText(Store.getMaxWait(this).toString())
        etCountry.setText(Store.getCountryCode(this))

        findViewById<Button>(R.id.btnPick).setOnClickListener {
            startActivity(Intent(this, ContactPickerActivity::class.java))
        }
        findViewById<Button>(R.id.btnClear).setOnClickListener {
            Store.recipients.clear()
            Store.results.clear()
            alicilariTazele()
            sonuclariTazele()
            tvProgress.text = "Liste temizlendi"
        }
        btnContacts.setOnClickListener {
            requestPermissions(arrayOf(Manifest.permission.READ_CONTACTS), izinKoduKisi)
        }
        btnNotif.setOnClickListener {
            if (Build.VERSION.SDK_INT >= 33) {
                requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), izinKoduBildirim)
            }
        }
        btnAccess.setOnClickListener {
            try {
                startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
            } catch (e: Exception) {
                toast("Erişilebilirlik ayarları açılamadı")
            }
        }
        btnOverlay.setOnClickListener { ustteGostermeIzinIste() }

        btnSend.setOnClickListener { gonder() }
        btnStop.setOnClickListener { durdur() }

        duymeleriGuncelle(false)
        alicilariTazele()
        sonuclariTazele()
    }

    override fun onResume() {
        super.onResume()
        if (!receiverKayitli) {
            val filtre = IntentFilter(SendService.ACTION_UPDATE)
            if (Build.VERSION.SDK_INT >= 33) {
                registerReceiver(receiver, filtre, Context.RECEIVER_NOT_EXPORTED)
            } else {
                registerReceiver(receiver, filtre)
            }
            receiverKayitli = true
        }
        // ContactPickerActivity'den dönüşte liste tazelenir.
        alicilariTazele()
        izinDurumunuGuncelle()
    }

    override fun onPause() {
        super.onPause()
        kaydiKaldir()
    }

    override fun onDestroy() {
        super.onDestroy()
        kaydiKaldir()
    }

    private fun kaydiKaldir() {
        if (receiverKayitli) {
            try {
                unregisterReceiver(receiver)
            } catch (e: Exception) {
                // yok sayılır
            }
            receiverKayitli = false
        }
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        izinDurumunuGuncelle()
        if (requestCode == izinKoduKisi && grantResults.isNotEmpty() &&
            grantResults[0] == PackageManager.PERMISSION_GRANTED
        ) {
            toast("Rehber izni verildi")
        }
    }

    // ------------------------------------------------------------- izinler

    private fun kisiIzniVar(): Boolean =
        checkSelfPermission(Manifest.permission.READ_CONTACTS) == PackageManager.PERMISSION_GRANTED

    private fun bildirimIzniVar(): Boolean {
        if (Build.VERSION.SDK_INT < 33) return true
        return checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
    }

    /** Erişilebilirlik servisimiz açık mı? */
    private fun erisilebilirlikAcik(): Boolean {
        return try {
            val acik = Settings.Secure.getString(
                contentResolver,
                Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES
            ) ?: return false
            val ben = packageName + "/" + WaAccessibilityService::class.java.name
            acik.split(':').any { it.equals(ben, true) }
        } catch (e: Exception) {
            false
        }
    }

    private fun izinDurumunuGuncelle() {
        val mesajlar = StringBuilder()
        val kisiEksik = !kisiIzniVar()
        val bildirimEksik = !bildirimIzniVar()
        val erisimEksik = !erisilebilirlikAcik()
        val ustteEksik = !ustteGostermeVar()

        if (kisiEksik) mesajlar.append("• Rehber izni yok: kişi seçemezsiniz.\n")
        if (bildirimEksik) mesajlar.append("• Bildirim izni yok: ilerlemeyi göremezsiniz.\n")
        if (erisimEksik) mesajlar.append("• Erişilebilirlik kapalı: gönder düğmesine basılamaz.\n")
        if (ustteEksik) mesajlar.append("• Üstte gösterme izni yok: WhatsApp arka planda açılamayabilir.\n")

        if (mesajlar.isEmpty()) {
            tvPerm.visibility = View.GONE
        } else {
            tvPerm.visibility = View.VISIBLE
            tvPerm.text = "Eksik izinler:\n$mesajlar"
        }
        btnContacts.visibility = if (kisiEksik) View.VISIBLE else View.GONE
        btnNotif.visibility = if (bildirimEksik) View.VISIBLE else View.GONE
        btnAccess.visibility = if (erisimEksik) View.VISIBLE else View.GONE
        btnOverlay.visibility = if (ustteEksik) View.VISIBLE else View.GONE
    }

    /** "Diğer uygulamaların üzerinde göster" izni var mı? (bazı üreticilerde hata verebilir) */
    private fun ustteGostermeVar(): Boolean {
        return try {
            Settings.canDrawOverlays(this)
        } catch (e: Exception) {
            false
        }
    }

    private fun ustteGostermeIzinIste() {
        try {
            val niyet = Intent(
                Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                Uri.parse("package:$packageName")
            )
            startActivity(niyet)
        } catch (e: Exception) {
            toast("Ayar ekranı açılamadı")
        }
    }

    // ------------------------------------------------------------ gönderim

    private fun gonder() {
        if (Store.recipients.isEmpty()) {
            toast("Önce rehberden kişi seçin")
            return
        }
        val mesaj = etMessage.text.toString()
        if (mesaj.isBlank()) {
            toast("Mesaj boş olamaz")
            return
        }
        var min = etMin.text.toString().trim().toIntOrNull() ?: 4
        var max = etMax.text.toString().trim().toIntOrNull() ?: 9
        if (min < 0) min = 0
        if (max < min) {
            max = min
            toast("En çok bekleme, en az beklemeden küçüktü; düzeltildi")
        }
        etMin.setText(min.toString())
        etMax.setText(max.toString())

        val ulke = etCountry.text.toString().trim().ifEmpty { "90" }
        Store.setMessage(this, mesaj)
        Store.setMinWait(this, min)
        Store.setMaxWait(this, max)
        Store.setCountryCode(this, ulke)

        Store.results.clear()
        sonuclariTazele()
        tvProgress.text = "Başlatılıyor..."

        try {
            val niyet = Intent(this, SendService::class.java).setAction(SendService.ACTION_START)
            startForegroundService(niyet)
            duymeleriGuncelle(true)
        } catch (e: Exception) {
            toast("Servis başlatılamadı: ${e.message}")
            duymeleriGuncelle(false)
        }
    }

    private fun durdur() {
        try {
            val niyet = Intent(this, SendService::class.java).setAction(SendService.ACTION_STOP)
            startService(niyet)
            tvProgress.text = "Durduruluyor..."
        } catch (e: Exception) {
            toast("Servise ulaşılamadı: ${e.message}")
        }
    }

    private fun duymeleriGuncelle(calisiyor: Boolean) {
        btnSend.isEnabled = !calisiyor
        btnStop.isEnabled = calisiyor
    }

    // -------------------------------------------------------------- listeler

    private fun alicilariTazele() {
        tvCount.text = "Seçili kişi: ${Store.recipients.size}"
    }

    private fun sonuclariTazele() {
        resultAdapter.notifyDataSetChanged()
    }

    private fun toast(mesaj: String) {
        Toast.makeText(this, mesaj, Toast.LENGTH_SHORT).show()
    }

    /** Sonuç listesi: item_queue.xml satırlarını doldurur. */
    private inner class ResultAdapter : BaseAdapter() {
        override fun getCount(): Int = Store.results.size

        override fun getItem(position: Int): Any = Store.results[position]

        override fun getItemId(position: Int): Long = position.toLong()

        override fun getView(position: Int, convertView: View?, parent: ViewGroup?): View {
            val gorunum = convertView ?: layoutInflater.inflate(R.layout.item_queue, parent, false)
            val r = Store.results[position]

            gorunum.findViewById<TextView>(R.id.queueName).text = r.name
            gorunum.findViewById<TextView>(R.id.queuePhone).text = r.phone

            val durum = gorunum.findViewById<TextView>(R.id.queueStatus)
            durum.text = if (r.detail.isEmpty()) r.status else "${r.status} • ${r.detail}"
            durum.setTextColor(
                if (r.status == "gönderildi") 0xFF2E7D32.toInt() else 0xFFC62828.toInt()
            )
            return gorunum
        }
    }
}
