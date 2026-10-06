# WhatsApp Toplu Mesaj Gönderici (Tauri + Rust)

Bu proje, **Tauri (Rust)** ve **Vanilla JS** kullanılarak geliştirilmiş, ultra hafif ve ban korumalı bir WhatsApp Web toplu mesaj gönderme uygulamasıdır. Tarayıcı otomasyonu (Selenium/Puppeteer) kullanmaz; doğrudan WhatsApp Web websocket protokolü üzerinden (`wa-rs` kütüphanesi ile) iletişim kurar.

## ✨ Özellikler

- **Rehber İçe Aktarma:** `.vcf` (iCloud VCard) ve `.csv` dosyalarından isim ve telefon numaralarını topluca çekme.
- **Otomatik Numara Düzeltme:** Eklediğiniz numaralar nasıl yazılırsa yazılsın (boşluklu, +90, 0555 vb.), sistem WhatsApp'ın kabul ettiği `90555...` formatına arka planda otomatik çevirir.
- **Gruplama ve Hızlı Seçim:** Ana listedeki kişileri bozmadan "Müşteriler", "VIP" gibi hızlı seçim grupları oluşturma ve tek tıkla seçme.
- **İsme Özel Hitap:** Mesaj kutusuna `/isim` yazdığınızda, mesaj giderken otomatik olarak kişinin rehberdeki adıyla değiştirilir. *(Örn: Merhaba /isim, nasılsın? -> Merhaba Ahmet, nasılsın?)*
- **Resimli Mesaj:** Metin mesajlarının yanına bilgisayarınızdan resim (`.png`, `.jpg`) ekleyerek medyalı gönderim yapabilme.
- **Anti-Ban Koruması:** WhatsApp spam filtrelerine takılmamak için her mesaj gönderimi arasında **4 ile 9 saniye arası rastgele** bir bekleme süresi uygulanır.

## 📥 İndirme (Windows .exe)

Windows bilgisayarlar için derlenmiş hazır `.exe` dosyasına [GitHub Actions](https://github.com/Soul-Art-000/whatsapp-sender-mac/actions) sekmesinden ulaşabilirsiniz. 
*Son başarılı "Build Windows App" işlemine tıklayıp en alttaki **Artifacts** bölümünden `windows-executable` dosyasını indirebilirsiniz.*

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
