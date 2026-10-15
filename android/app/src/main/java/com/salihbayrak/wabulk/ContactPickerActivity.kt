package com.salihbayrak.wabulk

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.os.Bundle
import android.provider.ContactsContract
import android.text.Editable
import android.text.TextWatcher
import android.view.View
import android.view.ViewGroup
import android.widget.BaseAdapter
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.ListView
import android.widget.TextView
import android.widget.Toast

/**
 * Rehberden çoklu kişi seçimi. Numaralar ülke koduyla normalize edilir ve
 * "Seçilenleri ekle" ile Store.recipients listesine aktarılır.
 */
class ContactPickerActivity : Activity() {

    // Listede gösterilen satır
    private class Row(val name: String, val phone: String) {
        var checked: Boolean = false
    }

    private val tumu = ArrayList<Row>()
    private val gorunen = ArrayList<Row>()
    private lateinit var adapter: RowAdapter
    private lateinit var btnAdd: Button

    private val izinKodu = 201
    private var izinIstendi = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_contact_picker)

        adapter = RowAdapter()
        val liste = findViewById<ListView>(R.id.listContacts)
        liste.adapter = adapter
        // Satıra dokunmak seçimi değiştirir (CheckBox tıklamayı yutmaz).
        liste.setOnItemClickListener { _, _, position, _ ->
            gorunen[position].checked = !gorunen[position].checked
            secimSayisiniGuncelle()
            adapter.notifyDataSetChanged()
        }

        btnAdd = findViewById(R.id.btnAdd)
        btnAdd.setOnClickListener { secilenleriEkle() }
        findViewById<Button>(R.id.btnCancel).setOnClickListener { finish() }

        val etSearch = findViewById<EditText>(R.id.etSearch)
        etSearch.addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) {}
            override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) {}
            override fun afterTextChanged(s: Editable?) {
                filtrele(s?.toString() ?: "")
            }
        })

        if (checkSelfPermission(Manifest.permission.READ_CONTACTS) == PackageManager.PERMISSION_GRANTED) {
            rehberiYukle()
        } else {
            izinIstendi = true
            requestPermissions(arrayOf(Manifest.permission.READ_CONTACTS), izinKodu)
        }
        secimSayisiniGuncelle()
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == izinKodu) {
            if (grantResults.isNotEmpty() && grantResults[0] == PackageManager.PERMISSION_GRANTED) {
                rehberiYukle()
            } else {
                Toast.makeText(this, "Rehber izni verilmedi", Toast.LENGTH_SHORT).show()
                rehberiYukle() // boş liste gösterir
            }
        }
    }

    /** Rehberi okur: aynı numara tekrar listelenmez, isme göre sıralanır. */
    private fun rehberiYukle() {
        val ulke = Store.getCountryCode(this)
        val yeni = ArrayList<Row>()
        val gorulen = HashSet<String>()

        try {
            val cursor = contentResolver.query(
                ContactsContract.CommonDataKinds.Phone.CONTENT_URI,
                arrayOf(
                    ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME,
                    ContactsContract.CommonDataKinds.Phone.NUMBER
                ),
                null,
                null,
                ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME + " ASC"
            )
            cursor?.use { c ->
                val adInd = c.getColumnIndex(ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME)
                val noInd = c.getColumnIndex(ContactsContract.CommonDataKinds.Phone.NUMBER)
                while (c.moveToNext()) {
                    val ad = if (adInd >= 0) c.getString(adInd) ?: "" else ""
                    val ham = if (noInd >= 0) c.getString(noInd) ?: "" else ""
                    val numara = Store.normalizePhone(ham, ulke)
                    if (numara.isNotEmpty() && gorulen.add(numara)) {
                        yeni.add(Row(ad.ifBlank { numara }, numara))
                    }
                }
            }
        } catch (e: Exception) {
            Toast.makeText(this, "Rehber okunamadı: ${e.message}", Toast.LENGTH_LONG).show()
        }

        yeni.sortWith(compareBy<Row>({ it.name.lowercase() }, { it.phone }))
        tumu.clear()
        tumu.addAll(yeni)
        filtrele((findViewById<EditText>(R.id.etSearch)).text.toString())
    }

    private fun filtrele(arama: String): Unit {
        val q = arama.trim().lowercase()
        gorunen.clear()
        for (r in tumu) {
            if (q.isEmpty() || r.name.lowercase().contains(q) || r.phone.contains(q)) {
                gorunen.add(r)
            }
        }
        adapter.notifyDataSetChanged()
        val bos = findViewById<TextView>(R.id.tvEmpty)
        bos.visibility = if (gorunen.isEmpty()) View.VISIBLE else View.GONE
        bos.text = if (izinIstendi && checkSelfPermission(Manifest.permission.READ_CONTACTS) != PackageManager.PERMISSION_GRANTED) {
            "Rehber izni gerekli"
        } else {
            "Kişi bulunamadı"
        }
    }

    private fun secilenleriEkle() {
        val secilenler = tumu.filter { it.checked }
            .map { Store.Recipient(it.name, it.phone) }
        if (secilenler.isEmpty()) {
            Toast.makeText(this, "Hiç kişi seçilmedi", Toast.LENGTH_SHORT).show()
            return
        }
        Store.addRecipients(secilenler)
        finish()
    }

    private fun secimSayisiniGuncelle() {
        val n = tumu.count { it.checked }
        btnAdd.text = "Seçilenleri ekle ($n)"
    }

    /** item_contact.xml satırlarını doldurur. */
    private inner class RowAdapter : BaseAdapter() {
        override fun getCount(): Int = gorunen.size

        override fun getItem(position: Int): Any = gorunen[position]

        override fun getItemId(position: Int): Long = position.toLong()

        override fun getView(position: Int, convertView: View?, parent: ViewGroup?): View {
            val gorunum = convertView ?: layoutInflater.inflate(R.layout.item_contact, parent, false)
            val row = gorunen[position]

            gorunum.findViewById<TextView>(R.id.contactName).text = row.name
            gorunum.findViewById<TextView>(R.id.contactPhone).text = row.phone
            gorunum.findViewById<CheckBox>(R.id.contactCheck).isChecked = row.checked
            return gorunum
        }
    }
}
