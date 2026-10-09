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

- **Tauri v1 `#[command]` argümanları JS'te camelCase**: `image_paths` ↔
  `imagePaths`, `contact_ids` ↔ `contactIds`. Yanlış yazarsan argüman `None`/hata
  olur ve sessizce bozulur.
- **`State` + blok sonu `MutexGuard` → E0597.** Şu kalıp derlenmez:
  ```rust
  let x = { let state = app.state::<S>(); state.m.lock().unwrap().clone() };
  ```
  Değeri ara değişkene al: `let v = ...lock().unwrap().clone(); v`.
  (Bu hata bir CI döngüsüne mal oldu; artık harness'te testi var.)
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

## Açık işler

- **Canlı gönderim uçtan uca test edilmedi** (geliştirme ortamında WhatsApp
  hesabı yok). Küçük bir listeyle denenmeli.
- `icons/icon.ico` girdileri Pillow yüzünden PNG formatında; Windows 10/11
  destekliyor ama spec için küçük boyutları DIB yapmak daha güvenli
  (`magick source.png -define icon:auto-resize=256,128,64,48,32,24,16 icon.ico`).
- Uzun gönderimler uygulama kapanınca kayboluyor; kuyruğu veritabanına yazıp
  kaldığı yerden devam etme fikri duruyor.
