# AGENTS.md — WhatsApp Sender (lively-oppenheimer)

Bu dosya, bu projede çalışacak ajanlar için bağlam ve tuzak notlarıdır.
Kullanıcı Türkçe konuşuyor; yanıtlar ve commit mesajları Türkçe.

## Proje nedir

**Tauri v1 (Rust) + vanilla JS** ile yazılmış WhatsApp toplu mesaj göndericisi.
Tarayıcı otomasyonu (Selenium/Puppeteer) **yok** — `wa-rs 0.2.0` ile doğrudan
WhatsApp Web websocket protokolü kullanılıyor. Mac'te geliştirilip Windows'a
derleniyor; repo: `Soul-Art-000/whatsapp-sender-mac`.

```
src/main.rs            Tauri komutları + wa-rs bağlantısı + toplu gönderim
frontend/index.html    Tüm arayüz (tek dosya, Tailwind CDN + jsDelivr QR lib)
icons/make_icon.py     Simgeyi üretir (PNG'ler + .ico + .icns)
tauri.conf.json        Sürüm + paketleme; version artırınca yeni sürüm çıkar
.github/workflows/     build-{windows,macos}.yml → derler + sürüm yayınlar
```

## Sık kullanılan komutlar

```bash
cargo tauri dev                       # geliştirme (Mac)
cargo tauri build                     # yerel derleme
python3 icons/make_icon.py            # simgeleri yeniden üret
git push origin master                # CI iki platformu derler + release yayınlar
gh run list --limit 5                 # koşuları izle
gh release view app-v<X> --json assets -q '.assets[].name'
gh run view <id> --log-failed         # derleme hatası
```

## DOĞRULAMA — bu projede en önemli şey

**Tauri katmanı Linux'ta derlenemez** (tauri v1 `webkit2gtk-4.0` ister, Debian
trixie'de yok). Bu yüzden `src/main.rs` içindeki Tauri'den bağımsız kod
`// ==== SHARED BEGIN ====` ve `// ==== SHARED END ====` işaretleri arasında
tutulur. Değişiklik yaptıktan sonra **mutlaka** şu şekilde doğrula:

1. Ayrı bir crate kur (`/opt/verify`), bağımlılıklar projeyle aynı olsun:
   `wa-rs`, `wa-rs-sqlite-storage`, `wa-rs-tokio-transport`, `wa-rs-ureq-http`,
   `rusqlite` (bundled), `csv`, `serde`, `tokio` (full). `tauri` **girmez**.
2. Crate'in `main.rs` = (projenin `use` satırları, tauri'siz) + SHARED bloğunun
   **birebir kopyası** + test harness. Blok kopyasını `python3` ile çıkar, elle
   yazma.
3. `cargo build && ./verify "<gerçek vcf yolu>"`.

Harness'te tutulan ve işe yarayan testler: `normalize_phone` senaryoları, gerçek
vCard ayrıştırma (1070 kayıt / 630 tekil numara beklenir), DB ekleme + kopya
sayımı, `/isim` + çoklu resim mesaj kurulumu, `map_event`, `interruptible_sleep`
(durdurma < 200 ms), `clear_contacts` SQL'i, `send_batch`/`find_unregistered`
future'lerinin `Send` olması, ödünç deseni (aşağıya bak).

Sandbox'ta `gh` yoksa: `apt-get install -y gh`. Jeton yoksa cihaz girişi kullan
(`github` skill → `references/auth.md`), PAT isteme.

## Kanıtlanmış tuzaklar

- **Tauri v1 `#[command]` parametrelerinde doc yorumu (`///`) derlenmez:** yalnızca
  `allow/cfg/deny/...` gibi yerleşik nitelikler kabul edilir → "allow, cfg, cfg_attr ... are
  the only allowed built-in attributes in function parameters". Parametre açıklaması
  gerekiyorsa düz `//` yorumu yaz. (Bu tuzak bir CI döngüsüne mal oldu; sandbox'ta Tauri
  katmanı derlenemediği için yalnızca macOS/Windows işinde görünür.)
- **Tauri v1 `#[command]` argümanları JS'te camelCase**: `attachment_paths` ↔
  `attachmentPaths`, `contact_ids` ↔ `contactIds`. Yanlış yazarsan argüman `None`/hata
  olur ve sessizce bozulur.
- **`State` + blok sonu `MutexGuard` → E0597.** Şu kalıp derlenmez:
  ```rust
  let x = { let state = app.state::<S>(); state.m.lock().unwrap().clone() };
  ```
  Değeri ara değişkene al: `let v = ...lock().unwrap().clone(); v`.
  (Bu hata bir CI döngüsüne mal oldu; artık harness'te testi var.)
- **Bağlantı kesme: `client.disconnect()` tek başına YETMEZ.** wa-rs kendi
  içinde yeniden bağlanır (`enable_auto_reconnect`); çağrı süren bir bağlanma
  denemesiyle yarışırsa etkisiz kalır ve kullanıcı "Kes"e basmasına rağmen
  bağlı kalır. Doğru sıra: `stop=true` → `enable_auto_reconnect=false` →
  bağlantı düşene kadar `disconnect()` döngüsü → `state.client = None`.
  "Çıkış" da bunu kullanır (`shared::logout_session`) ve bağlı değilken de
  oturum dosyasını siler; aksi hâlde bir sonraki açılışta kendiliğinden bağlanır.
- **wa-rs 0.2.0'da oturum kapatma API'si yok.** Ne `logout()`, ne
  `remove-companion-device` yardımcısı. Uydurma protokol paketi yazma; yapılan
  şey `client.disconnect()` + `wa_bot.db` silmek. Telefondaki "Bağlı cihazlar"
  kaydını kullanıcı kaldırır (arayüz bunu yazıyor).
- **`contacts().is_on_whatsapp()` yalnızca yanıtta gelen kullanıcıları döner.**
  Eksik gelen numarayı "kayıtlı değil" sayma — yalnızca açıkça
  `is_registered == false` olanları atla, yoksa rehberi yanlışlıkla budarsın.
- **`Event::Connected`** = bağlandı *ve* giriş yapıldı. Eski kod neredeyse her
  olayda "hazır" gönderiyordu; olayları tek tek eşle (`map_event`).
- **`TemporaryBan.expire` `chrono::TimeDelta`** (std `Duration` değil).
- **İki ayrı SQLite dosyası**: `contacts.sqlite` (kişiler/gruplar, uygulama) ve
  `wa_bot.db` (wa-rs oturumu). Çıkışta yalnız ikincisi silinir.
- **iCloud vCard'ları `item1.TEL;type=pref:` biçiminde gruplar**; satır başına
  `TEL` aramak numaraların yarısını kaybettirir. Katlanmış satırları da aç.
- **Ölü CDN**: `cdn.rawgit.com` kapalı. QR kütüphanesi jsDelivr'den geliyor.
- **wa-rs kendi içinde yeniden bağlanır**: `Disconnected` → `Connected` döngüsü
  arayüzde "Bağlı (Hazır)" ↔ "WhatsApp Bağla" zıplaması olarak görünür (kullanıcı
  bunu "bağlantı sorunu" diye bildirdi). Bu yüzden arayüz 5 saniyede bir
  `get_wa_status` ile tazelenir; olayları tek başına gerçek kaynak saymayın.
- **QR olayı kaçabilir**: bağlantı, arayüz hazır olmadan QR üretirse olay kaybolur.
  `AppState.last_qr` son QR'ı saklar, `connect_whatsapp` önce onu yayınlar
  (sunucu modundaki `last_qr` ile aynı davranış).
- **Boş QR kutusu genelde "eski oturum" demektir**: bağlıyken "WhatsApp Bağla" QR
  üretmez, pencere sonsuza kadar "Bağlanıyor..."da kalırdı. Artık zaten bağlıysa
  komut açık hata verir ve arayüz "Çıkış"ı her zaman gösterir (bozuk oturum ancak
  böyle temizlenir).
- **Günlük**: veri klasöründeki `app.log` (1 MB'da `app.log.1`). Bağlantı
  şikâyetlerinde ilk bakılacak yer; `log_line` + `fmt_utc` birim testli.
- **Tauri v1 allowlist'i derleme zamanında gömülür** (`tauri-codegen` →
  `#[module_command_handler]`). Arayüz yalnızca `invoke` (özel komut, allowlist
  istemez), `dialog.open` ve `event.listen` kullanır; `event`ın v1'de allowlist
  alanı **yok** (core). Bu yüzden config `all: false` + `dialog.open: true` olarak
  daraltıldı — varsayılan `all: true` + `shell: {all: true}` iken CDN'den gelen bir
  betik kabuk komutu çalıştırabiliyordu.
- **CSP bilinçli olarak `null`.** Tauri v1 CSP'ye kendi script'i için nonce ekler;
  nonce varken `'unsafe-inline'` yok sayıldığından index.html'deki satır içi
  script'ler bloklanabilir. CSP açılacaksa uygulama elle denenmeli (masaüstünde
  pencere boş/tepkisiz kalırsa ilk şüpheli budur).
- **Sunucu erişim anahtarı**: HTTP'de `X-Auth-Token` başlığı, WebSocket'te
  `?token=` (WS başlık taşıyamaz). Karşılaştırma `shared::token_eq` ile sabit
  zamanlıdır; `==` kullanmayın.
- **Anti-ban rastgeleliği**: `random_delay_secs` xorshift64* kullanır. `subsec_nanos()
  % n` gibi zaman tabanlı kısayollar macOS'ta (nanos 1000'in katı) değer uzayını
  daraltıyor — 4-9 yerine yalnızca 4/6/8 üretiyordu.
- **Mac klonunda hayalet mode değişiklikleri**: `/Volumes/T7` gibi harici diskte
  `100644 → 100755` gürültüsü olur. `git config core.fileMode false` bunu susturur
  (yerel ayar; `._*` dosyaları artık `.gitignore`'da).
- **SQLite: açık `SELECT` varken aynı bağlantıda `UPDATE`** "table is locked"
  verebiliyor. Satırları önce `Vec`'e topla, imleci düşür, sonra güncelle
  (bkz. `take_due_scheduled`).

## CI / yayın akışı

- İki workflow (Windows + macOS) `master`'a push'ta çalışır, `tauri-action` ile
  **aynı** sürüme yayınlar; `permissions: contents: write` şart.
- `paths` filtresi var: yalnızca `src/**`, `frontend/**`, `icons/**`, `Cargo.*`,
  `build.rs`, `tauri.conf.json`, `.github/workflows/**` değişince derler.
- **Yeni sürüm için `tauri.conf.json` (ve `Cargo.toml` + `Cargo.lock` içindeki
  kendi paket satırı) `version` artır.** Aynı sürümle push edersen asset adları
  çakışır ve yayın adımı hata verir. Etiket: `app-v<sürüm>`.
- macOS paketi **arm64** (Apple Silicon). Intel gerekiyorsa
  `--target universal-apple-darwin` ekle.
- macOS'ta `APPLE_SIGNING_IDENTITY: "-"` ad-hoc imza atar: paket "hasar görmüş"
  demez, sağ tık → Aç çalışır. Tam sessizlik için Apple Developer sertifikası +
  notarization gerekir (yok).
- Simge: `icon.icns` gerçek ICNS olmalı (eskiden 70 baytlık 1x1 PNG'ydi ve
  macOS'ta simge hiç görünmüyordu). Doğrulama: dmg'yi indir, `7z x`,
  `file .../Contents/Resources/icon.icns` → "Mac OS X icon" görmelisin.

## Arayüzü masaüstü derlemesi olmadan doğrulama (işe yarayan yöntem)

Tauri katmanı Linux'ta derlenemediği için arayüz değişikliklerini şöyle denedim:

1. Sunucu ikilisini derle ve token'sız başlat:
   `WA_BIND=127.0.0.1:8102 WA_DATA_DIR=/tmp/wa-test CARGO_TARGET_DIR=/opt/wcheck cargo build -p whatsapp-server`
2. Sayfayı çek: `curl -s http://127.0.0.1:8102/ -o /tmp/page.html`
3. Node harness: HTML'deki iki satır içi `<script>` bloğunu sırayla al, başına küçük
   bir DOM stub'ı ekle (`getElementById` **yalnızca HTML'de gerçekten olan id**'leri
   döndürsün — yazım hatalarını yakalar), `location.search='?api=http://127.0.0.1:8102'`
   ver, `WebSocket`'i hiç olay tetiklemeyen bir sınıfla değiştir ve sonuna test
   kodunu ekleyip `node` ile çalıştır. Köprü `fetch` ile gerçek sunucuya gider, yani
   Tauri komut adları/argümanları (camelCase) gerçekten denenmiş olur.
4. `fetch`/`getElementById` stub'ı sayesinde "Zamanla → listede görünüyor mu → İptal
   listeyi boşaltıyor mu" akışı uçtan uca doğrulanır; ağır DOM (querySelectorAll gibi)
   desteklenmediği için yalnızca kişi listesi render'ı hata verir, bu beklenen gürültüdür.

## Android sürümü (`android/`, Kotlin)

- **Neden protokol değil:** WhatsApp birincil telefonun kendisini bağlı cihaz
  olarak eklemez; bağlama, birincil telefonun kamerasıyla QR okutmayı gerektirir.
  Aynı telefonda "bağımsız gönderim" = telefonun kendi WhatsApp'ını
  **erişilebilirlik servisiyle otomatik kullanmak** (WA Bulk Pro vb. de böyle yapar).
- Masaüstündeki Rust/wa-rs çekirdeği burada kullanılmaz; Kotlin + platform API'leri
  (AndroidX **yok**, üçüncü parti bağımlılık yok, coroutine yok).
- Manifest izinleri: READ_CONTACTS, POST_NOTIFICATIONS, FOREGROUND_SERVICE(+DATA_SYNC),
  **SYSTEM_ALERT_WINDOW** (Android 10+ arka plandan WhatsApp'ı açabilmek için; erişilebilirlik
  servisi zaten bağlı olduğundan çoğu durumda gerekmez ama garanti için istenir).
- Gönderim döngüsü **ön plan servisinde** (WhatsApp öne gelirken süreç ölmesin).
- APK CI'da `assembleDebug` ile üretilir (debug anahtarıyla imzalı, sideload);
  ayrı bir sürüm etiketi kullanılır: `android-v<versionName>`.
- Gradle wrapper dosyası bilinçli olarak repoda yok (binary jar eklememek için);
  CI `gradle/actions/setup-gradle` ile Gradle 8.9 kurar.

## Ortam notları (sandbox)

- `terminal` bir Linux konteynerinde çalışır; `/workspace` = Mac'te
  `~/Documents/antigravity`. **macOS TCC**: `~/Documents` altındaki mevcut
  dosyalar okunamayabilir (EPERM); OrbStack'a Tam Disk Erişimi verilmeli.
- Sandbox zaman zaman yeniden başlıyor; uzun arka plan işleri (CI izleyiciler)
  kopabiliyor. Kritik işleri kısa tut veya sonucu elle kontrol et.

## Veri konumları ve sabitler

- Uygulama verisi (macOS): `~/Library/Application Support/com.whatsapp.sender/`
  → `contacts.sqlite`, `wa_bot.db`.
- Anti-ban ayarları (`src/main.rs`): mesajlar arası 4-9 sn rastgele, her 25
  kişide uzun mola, mesaj başına 30 sn üst sınır, 3 deneme, kopmada 90 sn
  yeniden bağlanma beklemesi, gönderim başında 20 sn bekleme.

## Zamanlanmış gönderim

Zamanlama **kuyruk**tur, bekleme değil: `add_scheduled` işi `scheduled` tablosuna
yazar, `scheduler_tick` (masaüstünde ve sunucuda 15 sn'lik tur) vakti geleni
`send_batch` ile yollar. `send_batch` bu yüzden **bloklamaz** — 0.4.0'daki
`start_at`/`wait_until` yaklaşımı bu yüzden kaldırıldı (saati gelene kadar
gönderim hattı kilitli kalıyordu ve uygulama kapanınca zamanlama uçuyordu).

- Tur yalnızca `AppState.busy` boşken çalışır; elle gönderim de aynı bayrağı
  kullanır, yani aynı anda tek gönderim olur (`scheduler_tick` meşgulse turu atlar,
  iş kuyrukta bekler).
- `sending` durumunda kalmış satır `take_due_scheduled` başında `pending`e çevrilir
  (gönderim ortasında kapanma) → mesaj kaybolmaz.
- Vakti `SCHEDULE_MISSED_GRACE_SECS` (2 saat) geçen iş kendiliğinden gönderilmez,
  "Kaçırıldı" olur; arayüzdeki **Şimdi gönder** `send_scheduled_now` ile tekrar
  kuyruğa alır.
- Masaüstünde pencere kapatılınca uygulama ölmesin diye tepsi var:
  `tauri` bağımlılığında `system-tray` özelliği + `tauri.conf.json` →
  `tauri.systemTray.iconPath` şart. `iconPath` paketlenen ikonlardan biri olmalı
  (`icons/32x32.png`); `icons/icon.png` `bundle.icon` listesinde **yok**. Pencereyi
  gizlemek için `WindowEvent::CloseRequested` + `api.prevent_close()`, geri getirmek
  için menüdeki "Pencereyi Aç".

## Açık işler

- **Canlı gönderim uçtan uca test edilmedi** (geliştirme ortamında WhatsApp
  hesabı yok). Küçük bir listeyle denenmeli.
- `icons/icon.ico` girdileri Pillow yüzünden PNG formatında; Windows 10/11
  destekliyor ama spec için küçük boyutları DIB yapmak daha güvenli
  (`magick source.png -define icon:auto-resize=256,128,64,48,32,24,16 icon.ico`).
- Zamanlanmış kuyruk yapıldı (`scheduled` tablosu + `scheduler_tick`); ayrıca
  canlı WhatsApp ile denenmesi gerekiyor (kuyruk mantığının birim testleri var).
- **Elle** başlatılan uzun gönderim hâlâ ortada kapanınca kayboluyor; onu da aynı
  kuyruğa yazıp kaldığı yerden sürdürmek açık bir iş.
