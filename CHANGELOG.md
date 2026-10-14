# Değişiklik Günlüğü

Bu dosya [Keep a Changelog](https://keepachangelog.com/tr/1.1.0/) düzenini ve
[Semantic Versioning](https://semver.org/lang/tr/) sürümlemeyi izler.

## [0.5.0] - 2026-10-14

### Added
- **Zamanlanmış mesaj kuyruğu (Gmail'deki "zamanlanmış" gibi):** iş `contacts.sqlite`
  içindeki `scheduled` tablosuna yazılır, gönderim 15 saniyelik arka plan turunda
  (`scheduler_tick`) yapılır. Bekleyen iş gönderim hattını **kilitlemez**: bu sırada
  başka bir liste hemen gönderilebilir.
- Arayüzde **Zamanlanmış Mesajlar** paneli: saat, durum (Bekliyor/Gönderiliyor/
  Gönderildi/Hata/Kaçırıldı), kişi sayısı ve önizleme; **İptal**, kaçırılan/hatalı iş
  için **Şimdi gönder**, bitmişler için **Geçmişi temizle**. Liste uygulama yeniden
  açıldığında da yerinde kalır.
- **Menü çubuğunda bekleme**: pencereyi kapatmak uygulamayı kapatmaz, gizler
  (`system-tray`); simgeye tıklayınca geri gelir, "Çıkış" tamamen kapatır.
  Zamanlanmış mesajlar pencere kapalıyken de gider.
- Sunucu modu aynı kuyruğu kullanır: sunucu 7/24 açıkken masaüstü kapalı olsa da
  zamanlanmış mesajlar yollanır.
- `src/shared.rs` birim testleri: kuyruğa yazma, geçmiş saat/boş iş reddi, işin bir kez
  alınması, gönderim ortasında kapanan işin yeniden kuyruğa girmesi, kaçırılma kuralı,
  iptal ve geçmiş temizliği.

### Changed
- **Zamanlama artık gönderimi bloklamıyor.** 0.4.0'daki `send_batch(start_at)` +
  `wait_until` yaklaşımı kaldırıldı: gönderim komutu saati gelene kadar bekliyordu,
  bu yüzden o sırada başka liste gönderilemiyordu ve uygulama kapanınca zamanlama
  kayboluyordu. Kuyruk yaklaşımı ikisini de çözer.
- **Aynı anda tek gönderim** (`AppState.busy`): elle gönderim sürerken ikinci bir toplu
  gönderim reddedilir; zamanlanmış iş kaybolmaz, kuyrukta bekler.
- Vakti 2 saatten fazla geçen işler kendiliğinden gönderilmez, "Kaçırıldı" olur.
- Gönderim ortasında kapanan iş `sending` kalır ve sonraki turda yeniden kuyruğa alınır.

## [0.4.0]

- **Zamanlanmış gönderim:** mesaj panelinden tarih/saat seçilir; gönderim o
  saatte başlar. Bekleme sırasında kalan süre gösterilir ve "Durdur" ile iptal
  edilir (hiçbir mesaj gitmez, tüm liste "kalanlar" olur). Uygulama açık kalmalı.
- **Bağlantı kesme/çıkış düzeltmesi:** "Kes" ve "Çıkış" istemciyi gerçekten
  durduruyor. Otomatik yeniden bağlanma kapatılıyor, bağlantı düşene kadar
  tekrar deneniyor ve istemci durumdan düşürülüyor; ayrıca süren gönderim
  durdurulur. "Çıkış" bağlı değilken de oturum dosyasını temizler.

## [0.2.0] - 2026-10-12

### Added
- **Sunucu modu (headless)**: aynı çekirdek `whatsapp-server` ikilisiyle HTTP API +
  WebSocket üzerinden sunulur; arayüz ikilinin içine gömülüdür, Tauri/WebKitGTK
  gerekmez. `cargo build --release --no-default-features --features server`
- **Docker imajı**: çok aşamalı `Dockerfile` + `docker-compose.yml`; ilk açılışta erişim
  anahtarı üretilir; GitHub Actions imajı GHCR'a yayınlar ve duman testi çalıştırır.
- **GitHub Pages arayüzü**: statik arayüz Pages'te barındırılır, `?api=https://sunucu`
  ile uzaktaki backend'e bağlanır (CORS destekli).
- Erişim anahtarı (`WA_TOKEN`) ve yalnızca yükleme klasöründen okuma (yol kaçışı engeli).

### Changed
- Tauri'den bağımsız mantık `src/shared.rs`'e taşındı; masaüstü ve sunucu tek çekirdeği
  paylaşır (gönderim/anti-ban/import mantığı bölünmez).
- `tauri` bağımlılığı `desktop` özelliği arkasına alındı; sunucu hedefi Linux'ta
  WebKitGTK olmadan derlenir.

## [0.2.1] - 2026-10-12

### Fixed
- **Arayüz hiç çalışmıyordu**: sunucu köprüsündeki `return` (blok düzeyinde) sözdizimi
  hatası veriyor ve tarayıcı tüm betiği reddediyordu. IIFE içine alındı; hem masaüstü
  hem sunucu arayüzü doğrulandı (tarayıcıda 631 kişilik liste + QR akışı).

### Added
- `/healthz` sağlık ucu (Docker HEALTHCHECK) ve `/` sayfasının anahtarsız açılması;
  veri uçları `WA_TOKEN` ister.
- Kaçırılan QR yeniden gösterilir: sunucu arayüzden önce bağlandıysa son QR saklanır,
  arayüz bağlanınca ve "WhatsApp Bağla"ya basınca yeniden iletilir.
- Köprü, 401 alınca erişim anahtarını sorar ve tarayıcıda saklar.

## [0.3.0] - 2026-10-13

### Added
- **Belge gönderme**: resimlerin yanında her tür dosya (`.pdf`, `.doc(x)`, `.xls(x)`,
  `.ppt(x)`, `.txt`, `.csv`, `.zip`, `.rar`, `.7z`, uzantısız/tanınmayanlar) eklenebilir.
  Resimler önizlemeli gider, diğerleri WhatsApp belgesi (indirilebilir ek) olarak;
  alıcı gerçek dosya adını görür. İlk ek yazılan mesajı altyazı olarak taşır.
- Ek kararı uzantıya göre çekirdekte verilir (`kind_for`), ekler kişi başına değil
  gönderim başına bir kez yüklenir — belgeler `MediaType::Document` ile şifrelenip yüklenir.
- `src/shared.rs` içine birim testleri: ek türü/mime eşlemesi, dosya adı ayıklama,
  belge mesajı kurulumu, karışık ek sırası ve `/isim` altyazısı.

### Changed
- `send_whatsapp_messages` parametresi `imagePaths` → `attachmentPaths` (sunucu API'si ve
  arayüz köprüsü dahil). Arayüzdeki ekleme düğmesi "Resim veya Belge Ekle" oldu; seçilen
  dosyalar listede 🖼/📄 olarak ayrılır.
- Sunucu modunda yüklenen dosya adında boşluk ve parantez korunur (alıcıya görünen ad).
- CI artık `whatsapp-sender` kütüphane testlerini de çalıştırır
  (`cargo test --no-default-features -p whatsapp-sender --lib`).

## [Unreleased]

### Added
- İlk sürüm öncesi çalışmalar.
