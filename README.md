# WhatsApp Toplu Mesaj Gönderici (Tauri + Rust)

Bu proje, **Tauri (Rust)** ve **Vanilla JS** kullanılarak geliştirilmiş, ultra hafif ve ban korumalı bir WhatsApp Web toplu mesaj gönderme uygulamasıdır. Tarayıcı otomasyonu (Selenium/Puppeteer) kullanmaz; doğrudan WhatsApp Web websocket protokolü üzerinden (`wa-rs` kütüphanesi ile) iletişim kurar.

## ✨ Özellikler

- **Rehber İçe Aktarma:** `.vcf` (iCloud VCard) ve `.csv` dosyalarından isim ve telefon numaralarını topluca çekme. iCloud'un `item1.TEL;type=pref:` biçimindeki gruplanmış numaraları ve katlanmış uzun satırları da okunur.
- **Otomatik Numara Düzeltme:** Eklediğiniz numaralar nasıl yazılırsa yazılsın (boşluklu, +90, 0555, 0090... vb.) hem kayıt sırasında hem gönderim sırasında WhatsApp'ın kabul ettiği `90555...` formatına çevrilir. Aynı numara ikinci kez eklenmez.
- **Gruplama ve Hızlı Seçim:** Ana listedeki kişileri bozmadan "Müşteriler", "VIP" gibi hızlı seçim grupları oluşturma ve tek tıkla seçme.
- **İsme Özel Hitap:** Mesaj kutusuna `/isim` yazdığınızda, mesaj giderken otomatik olarak kişinin rehberdeki adıyla değiştirilir. *(Örn: Merhaba /isim, nasılsın? -> Merhaba Ahmet, nasılsın?)*
- **Resimli Mesaj:** Metin mesajlarının yanına bilgisayarınızdan resim (`.png`, `.jpg`, `.webp`) ekleyerek medyalı gönderim yapabilme. Resim bir kez yüklenir, tüm kişilere tekrar tekrar yüklenmez.
- **Numara Doğrulama:** Gönderimden önce (isteğe bağlı, varsayılan açık) numaralar WhatsApp'a sorulur; kayıtlı olmayanlar hiç mesaj gönderilmeden atlanır. Bu hem boşa gönderimi hem de ban riskini azaltır.
- **Bağlantı Yönetimi:** Uygulama açılışta kayıtlı oturumla kendiliğinden bağlanır (QR kod her seferinde okutulmaz). Bağlantı koparsa wa-rs artan beklemeyle kendi kendine yeniden bağlanır; gönderim sırasında kopma olursa sistem mesajı yakmayıp bağlantının geri gelmesini bekler ve kaldığı yerden devam eder.
- **Anti-Ban Koruması:** Her mesaj arasında **4-9 saniye rastgele** bekleme, her 25 kişide bir uzun mola. Tek bir mesaj için 30 saniye üst sınır vardır; takılan gönderim tüm listeyi kilitlemez.
- **Durdurma ve İlerleme:** Gönderim sırasında ilerleme çubuğu ve kalan kişi bilgisi görünür; **Durdur** bekleme sırasında bile anında keser.
- **Hata Toleransı ve Tekrar Deneme:** Geçici hatada 3 kez denenir, kalıcı hatada kalanlara devam edilir. Biten işlemde kaç kişiye gittiği, kaç kişide hata olduğu ve kaç numaranın atlandığı bildirilir; **Başarısızları tekrar dene** ile sadece hatalı olanlar yeniden gönderilir.

## 📥 İndirme

- **Windows:** Derlenmiş `.exe` için [GitHub Actions](https://github.com/Soul-Art-000/whatsapp-sender-mac/actions) → *Build Windows App* → son başarılı çalışmanın **Artifacts** bölümünden `windows-executable`.
- **macOS:** *Build macOS App* işinin **Artifacts** bölümünden `macos-app` (`.dmg` / `.app`).

## 🚀 Geliştirme (Mac / Linux / Windows)

Kodu kendi bilgisayarınızda derlemek veya geliştirmek için sisteminizde **Rust** ve **Node.js** (Tauri bağımlılıkları için) kurulu olmalıdır.

### Geliştirici Ortamını Başlatma:
```bash
# Projeyi klonlayın
git clone https://github.com/Soul-Art-000/whatsapp-sender-mac.git
cd whatsapp-sender-mac

# Geliştirici modunda çalıştırın (Anında yenileme)
cargo tauri dev
```

### Üretime Hazır Derleme (Build):
```bash
cargo tauri build
```
Derleme tamamlandığında işletim sisteminize uygun kurulum dosyası (`.dmg`, `.exe` veya `.AppImage`) `target/release/bundle` klasörü altında oluşacaktır.

## ⚠️ Uyarı
Bu uygulama tamamen açık kaynaklı ve kişisel kullanım/öğrenim amaçlıdır. Çok kısa sürede binlerce mesaj atmak WhatsApp kullanım koşullarına aykırıdır ve hesabınızın kapatılmasına yol açabilir. Sorumluluk kullanıcıya aittir. Yüksek hacimli gönderimleri 20-30 kişilik bloklara bölerek yapmanız tavsiye edilir.
