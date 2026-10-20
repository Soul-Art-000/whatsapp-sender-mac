# Değişiklik Günlüğü

Bu dosya [Keep a Changelog](https://keepachangelog.com/tr/1.1.0/) düzenini ve
[Semantic Versioning](https://semver.org/lang/tr/) sürümlemeyi izler.

## [0.5.7] - 2026-10-20

### Added
- **Çıkış sonucu arayüzde gösteriliyor.** `logout_whatsapp` artık bir rapor
  döndürür ("bağlantı: kesildi · oturum dosyası: silindi") ve QR penceresinde
  görünür. "Çıktı mı çıkmadı mı" sorusu tahmin yerine ölçümle yanıtlanır;
  aynı satır `app.log`'a da yazılır.

## [0.5.6] - 2026-10-20

### Fixed
- **"Çıkış" basılınca arayüz "bağlı" kalıyor, QR istenemiyordu.** Düğme
  sonucu yalnızca `whatsapp_logged_out` olayına bırakıyordu; olay kaçınca
  (veya sıra yarışında düşünce) görünüm değişmiyordu. Artık düğme kendi
  başarısından sonra durumu doğrudan `offline` yapar, QR penceresini
  "QR kod bekleniyor" durumuna alır ve "Bağlan"ı etkinleştirir.
- `spawn_connect` erken dönüşleri ("zaten bağlı"/"bağlanma sürüyor") canlı
  bağlanma turunun `running` jetonunu eziyordu → o turun kapanışı yeni
  bağlantıyı düşürebiliyordu.
- `connecting` bayrağı kapanışta önce düşürülüyor; "Çıkış" artık 15 saniye
  boşuna beklemiyor.
- Abort edilen `run()` görevi bekleniyor: oturum dosyası kilitli kalmıyor.
- Çıkış, dosya temizliği başarısız olsa bile başarılı sayılır (bağlantı gitti).
- `lastStatus` tanım-kullanım sırası hatası (TDZ) giderildi.
- `frontend/exit-flow.test.js` eklendi ve CI'ya bağlandı: sahte DOM + invoke
  ile `index.html`'in kendi JS'i çalıştırılıp çıkış akışı doğrulanıyor.
- Clippy `too_many_arguments` (CI kırmızısı) giderildi.

## [0.5.5] - 2026-10-20

### Added
- **Tek görüntülük (view once) resim gönderimi.** Gönderim ve zamanlama
  panelinde "Resimleri tek görüntülük gönder" onay kutusu eklendi; işaretliyse
  eklenen görseller `viewOnce` bayrağıyla gider (alıcı yalnızca bir kez açar).
  Zamanlanmış işlerde tercih `scheduled.view_once` kolonunda saklanır (eski
  veritabanları açılışta otomatik güncellenir). Belgeler bu bayrağı taşımaz.

## [0.5.4] - 2026-10-20

### Fixed
- **"Çıkış" sonrası QR gelmiyordu ve oturum geri geliyordu.** Çıkış, oturum
  dosyasını silmeye çalışıp başarısız olunca (DB kilidi / izin) hata dönüyor,
  `client` zaten `None` olduğu için hiçbir şey değişmiyordu; arayüz "oturum
  silindi" yazıyor ama WhatsApp bağlı kalıyor ve yeni QR üretilmiyordu.
  Artık dosya silinemezse boşaltılır: oturum her hâlükârda geçersiz olur,
  sonraki bağlantıda yeni cihaz kimliği üretilip QR gösterilir.
- `LoggedOut` olayı artık `whatsapp_logged_out` olarak yayınlanıyor (eskiden
  `whatsapp_error` idi); QR penceresi bu olayda yeniden "QR bekleniyor"
  durumuna dönüyor.
- Çıkış sonucu `app.log`'a yazılıyor ("oturum silindi" / "çıkış HATASI: ...").

## [0.5.3] - 2026-10-20

### Fixed
- **"Çıkış" / "Kes" WhatsApp bağlantısını kesmiyordu.** İki kök neden vardı:
  1. `disconnect_session`, `client` alanı boşsa (`None`) hemen
     "Zaten bağlı değil." dönüp **hiçbir şey yapmıyordu**; oysa
     `spawn_connect` ile bağlanma/QR turu arka planda hâlâ sürebiliyor.
     Artık işlem her durumda ilerler.
  2. `bot.run()` ayrı bir görev açar ve bu görevin handle'ı tutulmuyordu.
     Oturum yeniden bağlanma döngüsünü kendi sürdürdüğü için, istemci nesnesi
     düşürülse bile WhatsApp bağlı kalıyordu. Artık handle `AppState`'te
     saklanıyor ve "Kes"/"Çıkış" onu `abort` ediyor.
- Eski bağlanma turunun kapanış temizliği artık yeni bağlantıya dokunmuyor
  (`running` jetonu): çıkıştan hemen sonra arayüz "offline"a düşüp bağlantıyı
  bozması engellendi. Aynı düzeltme sunucu modunda da yapıldı.

## [0.5.2] - 2026-10-14

### Fixed
- **"WhatsApp Bağla" bağlıyken hiçbir şey yapmıyordu**: komut sessizce dönüyor,
  pencere "Bağlanıyor..." ve boş QR kutusuyla kalıyordu. Artık zaten bağlıysa
  sebebi açıkça söyleniyor ("önce Çıkış ile oturumu silin"), bağlantı sürerken
  üretilmiş bir QR varsa tekrar gösteriliyor (`last_qr`, sunucu modundaki gibi).
- **"Çıkış" düğmesi çevrimdışıyken gizleniyordu**: eski/bozuk oturum dosyası
  olduğunda kullanıcının QR ekranına düşmesinin hiçbir yolu kalmıyordu. Artık
  "Çıkış" her zaman görünür ("Kes" yalnızca canlı oturumda).
- **Durum göstergesi gerçeği söylüyor**: arayüz 5 saniyede bir bağlantı durumunu
  tazeliyor ve `whatsapp_ready` geldiğinde eski "Bağlantı kapandı." yazısı
  temizleniyor (eskiden "Bağlı (Hazır)" ile bu yazı aynı anda görünebiliyordu).
- QR penceresi 12 saniye içinde QR gelmezse ne yapılacağını söylüyor.

### Added
- **Uygulama günlüğü**: her olay ve bağlantı adımı
  `~/Library/Application Support/com.whatsapp.sender/app.log` dosyasına yazılıyor
  (1 MB'da `app.log.1`'e devredilir). Bağlantı sorunlarını uzaktan teşhis etmek için.

## [0.5.1] - 2026-10-14

### Güvenlik
- **Tauri allowlist'i daraltıldı.** Eskiden `all: true` + `shell: {all: true}` idi;
  arayüz yalnızca `invoke`, `dialog.open` ve `event.listen` kullandığı için artık
  `all: false` + `dialog.open: true`. Böylece arayüzün yüklediği uzak betiklerden
  biri (CDN) ele geçirilse bile kabuk komutu çalıştırılamaz.
- **Erişim anahtarı artık başlıkla gönderiliyor**: HTTP çağrıları `X-Auth-Token`
  kullanır (token sunucu/proxy loglarına ve tarayıcı geçmişine düşmez). WebSocket
  başlık taşıyamadığı için sorgu parametresi orada korunur; sunucu ikisini de kabul
  eder.
- **Token karşılaştırması sabit zamanlı** (`shared::token_eq`): `==` ilk farklı
  baytta döndüğü için zamanlama sızıntısı veriyordu.
- `.gitignore`: `._*` AppleDouble dosyaları (harici disk artıkları) eklendi.
- **Bağımlılık taraması** (OSV/RUSTSEC, 726 paket): `rustls 0.23.45 → 0.23.46`
  yükseltildi (4 kayıt kapatıldı; biri kimlik doğrulamasız ~1 GiB ayırma DoS'u).
  Kalan 10 kayıt yukarı akışa bağlı ve bilinçli olarak kabul edildi: 5'i
  "unmaintained" bildirimi, 2'si Linux/GTK'ya özel `glib` (macOS/Windows paketinde
  yok), `h2 0.3` (tauri v1 → hyper 0.14 zinciri; uygulama Tauri HTTP API'sini hiç
  kullanmıyor) ve `rand 0.7` (özel logger şartı). CI'ya kırmayan bir
  `rustsec/audit-check` adımı eklendi.

### Fixed
- **Anti-ban beklemesi gerçekten rastgele değildi**: `subsec_nanos() % 6` macOS'ta
  nanos 1000'in katı geldiği için yalnızca 4, 6 veya 8 saniye üretiyordu; 5, 7 ve 9
  hiç çıkmıyordu. Yerine küçük bir xorshift64* jeneratörü kondu (ek bağımlılık yok).
  Birim testi 2000 çağrıda 4-9 arası altı değerin de üretildiğini doğrular.

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
