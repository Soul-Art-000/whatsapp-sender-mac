//! Çekirdek mantık — Tauri'den bağımsız.
//!
//! Aynı kod iki yerden kullanılır: masaüstü uygulaması (`src/main.rs`, Tauri
//! komut katmanı) ve sunucu ikilisi (`src/bin/whatsapp-server.rs`, HTTP/WS
//! katmanı). Böylece gönderim/anti-ban/import mantığı tek yerde kalır.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wa_rs::bot::Bot;
use wa_rs::types::events::Event;
use wa_rs_sqlite_storage::SqliteStore;
use wa_rs_tokio_transport::TokioWebSocketTransportFactory;
use wa_rs_ureq_http::UreqHttpClient;

/// Uygulama durumu (masaüstünde Tauri `State`, sunucuda `Arc`).
pub struct AppState {
    pub db: Mutex<Connection>,
    pub client: Mutex<Option<Arc<wa_rs::Client>>>,
    /// Gönderim sırasında "Durdur" için.
    pub stop: Arc<AtomicBool>,
    /// Aynı anda iki kez bağlanmayı engeller.
    pub connecting: Arc<AtomicBool>,
    /// Şu anda bir toplu gönderim (elle ya da zamanlanmış) sürüyor mu.
    /// İki gönderimin aynı anda WhatsApp istemcisini kullanmasını engeller.
    pub busy: Arc<AtomicBool>,
    /// En son üretilen QR kodu. Arayüz açılmadan/bağlanmadan önce üretilirse
    /// kaybolmasın diye saklanır (sunucu modunda zaten böyleydi).
    pub last_qr: Mutex<Option<String>>,
    /// Çalışan `run_bot` turunun canlılık anahtarı; "Kes"/"Çıkış" `false` yapar.
    pub running: Mutex<Option<Arc<AtomicBool>>>,
    /// `bot.run()` görevinin handle'ı; "Kes"/"Çıkış" bunu abort eder.
    pub stop_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl AppState {
    pub fn new(db: Connection) -> Self {
        AppState {
            db: Mutex::new(db),
            client: Mutex::new(None),
            stop: Arc::new(AtomicBool::new(false)),
            connecting: Arc::new(AtomicBool::new(false)),
            busy: Arc::new(AtomicBool::new(false)),
            last_qr: Mutex::new(None),
            running: Mutex::new(None),
            stop_handle: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(Serialize)]
pub struct Group {
    pub name: String,
    pub contact_ids: Vec<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Contact {
    pub id: i64,
    pub name: String,
    pub phone: String,
}

#[derive(Serialize, Clone)]
pub struct SendSummary {
    pub total: usize,
    pub sent: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Gönderilemeyenler + hiç denenmemişler (Durdur / bağlantı koptu).
    /// "Tekrar dene" tam olarak bu listeyi gönderir.
    pub pending: Vec<Contact>,
}

#[derive(Serialize, Clone)]
pub struct ImportResult {
    pub added: usize,
    pub duplicates: usize,
}

// ---------------------------------------------------------------- telefon

/// Ham bir telefon numarasını WhatsApp'ın beklediği `905551112233` biçimine
/// çevirir. Numarayı tanıyamazsa `None` döner.
///
/// Desteklenen girdiler: `+90 555 123 45 67`, `0555 123 45 67`,
/// `5551234567`, `00905551234567`, `(053) 785 377 70`.
pub fn normalize_phone(raw: &str) -> Option<String> {
    let mut digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    // 00 ile başlayan uluslararası ön ek
    if let Some(rest) = digits.strip_prefix("00") {
        digits = rest.to_string();
    }
    // Ulusal ön ek (0)
    if let Some(rest) = digits.strip_prefix('0') {
        digits = rest.to_string();
    }
    // Ülke kodu olmadan yazılmış 10 haneli numara -> 90 ekle
    if digits.len() == 10 {
        digits = format!("90{}", digits);
    }
    if !(11..=15).contains(&digits.len()) {
        return None;
    }
    Some(digits)
}

// ------------------------------------------------------------------ import

/// vCard (RFC 6350 / iCloud) içeriğini `(isim, telefon)` çiftlerine ayırır.
///
/// Elle yazılmış eski sürüm yalnızca satırı `TEL` ile başlayan numaraları
/// alıyordu; iCloud ise numaraları `item1.TEL;type=pref:` biçiminde
/// gruplandırıyor. Bu yüzden rehberin yarısı sessizce kayboluyordu.
pub fn parse_vcf(content: &str) -> Vec<(String, String)> {
    // 1) Katlanmış (folded) uzun satırları birleştir.
    let mut lines: Vec<String> = Vec::new();
    for raw in content.split('\n') {
        let line = raw.trim_end_matches('\r');
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = lines.last_mut() {
                last.push_str(line.trim_start_matches([' ', '\t']));
                continue;
            }
        }
        lines.push(line.to_string());
    }

    let mut out: Vec<(String, String)> = Vec::new();
    let mut current_name = String::new();

    for line in &lines {
        let Some(colon) = line.find(':') else {
            continue;
        };
        let head = &line[..colon];
        let value = &line[colon + 1..];

        // "item1.TEL;type=pref" -> "TEL"
        let key = head.split(';').next().unwrap_or("");
        let key = key.rsplit('.').next().unwrap_or(key);

        match key.to_ascii_uppercase().as_str() {
            "BEGIN" => current_name.clear(),
            "FN" => {
                let n = unescape_vcard(value);
                if !n.is_empty() {
                    current_name = n;
                }
            }
            "TEL" => {
                if let Some(phone) = normalize_phone(value) {
                    out.push((current_name.clone(), phone));
                }
            }
            _ => {}
        }
    }
    out
}

/// CSV içeriğini `(isim, telefon)` çiftlerine ayırır.
pub fn parse_csv(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(false)
        .from_reader(content.as_bytes());
    for record in rdr.records().flatten() {
        let mut name = String::new();
        let mut phone: Option<String> = None;
        for field in record.iter() {
            match normalize_phone(field) {
                Some(p) => {
                    if phone.is_none() {
                        phone = Some(p);
                    }
                }
                None => {
                    if name.is_empty() && !field.trim().is_empty() {
                        name = field.trim().to_string();
                    }
                }
            }
        }
        if let Some(p) = phone {
            out.push((name, p));
        }
    }
    out
}

pub fn unescape_vcard(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push(' '),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// `(isim, telefon)` çiftlerini veritabanına ekler. Aynı numara zaten varsa
/// atlar ve sayar (eski sürüm `INSERT OR IGNORE`ın 0 satır etkilediğini
/// görmezden gelip yanlış "şu kadar kişi eklendi" mesajı veriyordu).
pub fn insert_contacts(
    tx: &rusqlite::Transaction<'_>,
    pairs: &[(String, String)],
) -> rusqlite::Result<ImportResult> {
    let mut added = 0;
    let mut duplicates = 0;
    for (name, phone) in pairs {
        let name = if name.trim().is_empty() {
            "İsimsiz"
        } else {
            name.trim()
        };
        let changed = tx.execute(
            "INSERT OR IGNORE INTO contacts (name, phone) VALUES (?1, ?2)",
            params![name, phone],
        )?;
        if changed == 0 {
            duplicates += 1;
        } else {
            added += 1;
        }
    }
    Ok(ImportResult { added, duplicates })
}

// ------------------------------------------------------------------ gönderim

/// Numara doğrulama isteklerinin kaçar kaçar yapılacağı.
pub const VALIDATE_CHUNK: usize = 50;
/// Tek bir mesaj için üst sınır; takılan gönderim tüm listeyi kilitlemesin.
pub const SEND_TIMEOUT_SECS: u64 = 30;
/// Gönderimin başında bağlantı için beklenecek süre (yoksa hızlı hata ver).
pub const CONNECT_WAIT_START_SECS: u64 = 20;
/// Gönderim ortasında bağlantı koparsa yeniden bağlanma için beklenecek süre.
pub const CONNECT_WAIT_SECS: u64 = 90;
/// Geçici hatada mesaj başına deneme sayısı.
pub const MAX_SEND_ATTEMPTS: u32 = 3;
/// Kaç mesajda bir uzun mola verileceği.
pub const BREAK_EVERY: usize = 25;

/// 4-9 saniye arası rastgele anti-ban beklemesi.
///
/// Eski sürüm `subsec_nanos() % 6` kullanıyordu; macOS'ta nanos 1000'in katı
/// geldiği için sonuç yalnızca {0, 2, 4} — yani 4, 6 veya 8 saniye — olabiliyordu.
/// 5, 7 ve 9 hiç üretilmiyor, "rastgele" desen 3 değere sıkışıyordu. Aşağıdaki
/// küçük xorshift64* jeneratörü bunu düzeltir (ek bağımlılık yok).
pub fn random_delay_secs() -> u64 {
    4 + (next_rand() % 6)
}

/// Süreç içi rastgelelik durumu (tohum ilk çağrıda zamandan + pid'den gelir).
static RNG_STATE: AtomicU64 = AtomicU64::new(0);

/// xorshift64* — anti-ban beklemesi için fazlasıyla yeterli, dağılımı düzgün.
fn next_rand() -> u64 {
    let mut s = RNG_STATE.load(Ordering::Relaxed);
    if s == 0 {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ ((std::process::id() as u64) << 32)
            ^ (&RNG_STATE as *const AtomicU64 as u64);
        s = if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        };
    }
    let mut x = s;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    RNG_STATE.store(x, Ordering::Relaxed);
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// Bağlantı sorunlarını teşhis için uygulama günlüğü.
///
/// Masaüstü uygulamasında pencere konsolu yok; olaylar buraya yazılır:
/// `<veri klasörü>/app.log`. 1 MB'ı aşınca `app.log.1` olarak devredilir.
pub fn log_line(app_dir: &Path, msg: &str) {
    let path = app_dir.join("app.log");
    if std::fs::metadata(&path)
        .map(|m| m.len() > 1_000_000)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, app_dir.join("app.log.1"));
    }
    let line = format!("{} {}\n", fmt_utc(now_unix()), msg);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

/// UNIX saniyesini okunabilir UTC damgasına çevirir: `2026-10-14 15:42:07`.
///
/// Tarih kütüphanesi (chrono) bağımlılığı eklememek için civil-from-days
/// algoritması kullanılır (Howard Hinnant).
pub fn fmt_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, m, s)
}

/// Sabit zamanlı token karşılaştırması.
///
/// `==` ilk farklı baytta döndüğü için zamanlama sızıntısı verir; ağ üzerinden
/// ölçmek zor olsa da doğrusu bedava. Sunucu erişim anahtarını bununla doğrular.
pub fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// "Durdur" basıldığında hemen uyanan bekleme. `false` = durduruldu.
pub async fn interruptible_sleep(secs: u64, stop: &AtomicBool) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return true;
        }
        tokio::time::sleep((deadline - now).min(Duration::from_millis(500))).await;
    }
}

/// Şu anki UNIX zamanı (saniye).
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "905551234567:12@s.whatsapp.net" -> "905551234567"
pub fn jid_user(jid: &wa_rs::Jid) -> String {
    jid.to_string()
        .split('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string()
}

/// WhatsApp'ta kayıtlı OLMAYAN numaraları döndürür.
///
/// Sadece sunucunun açıkça "kayıtlı değil" dediği numaralar döner; yanıt
/// gelmeyenler ve doğrulamanın kendisi başarısız olan durumlar boş küme
/// döndürür ki kimse yanlışlıkla atlanmasın.
pub async fn find_unregistered<F: Fn(String) + Send>(
    client: &Arc<wa_rs::Client>,
    contacts: &[Contact],
    stop: &AtomicBool,
    emit: &F,
) -> std::collections::HashSet<String> {
    let mut unregistered = std::collections::HashSet::new();
    let mut checked = 0usize;

    for chunk in contacts.chunks(VALIDATE_CHUNK) {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if !(client.is_connected() && client.is_logged_in())
            && client
                .wait_for_connected(Duration::from_secs(CONNECT_WAIT_SECS))
                .await
                .is_err()
        {
            emit("Numara doğrulaması için bağlantı kurulamadı, doğrulama atlanıyor.".to_string());
            return std::collections::HashSet::new();
        }

        let phones: Vec<&str> = chunk.iter().map(|c| c.phone.as_str()).collect();
        match client.contacts().is_on_whatsapp(&phones).await {
            Ok(results) => {
                for r in results {
                    if !r.is_registered {
                        unregistered.insert(jid_user(&r.jid));
                    }
                }
            }
            Err(e) => {
                emit(format!(
                    "Numara doğrulaması yapılamadı ({}), doğrulama atlanıyor.",
                    e
                ));
                return std::collections::HashSet::new();
            }
        }

        checked += chunk.len();
        emit(format!(
            "Numaralar kontrol ediliyor... ({}/{})",
            checked,
            contacts.len()
        ));
        if !interruptible_sleep(1, stop).await {
            break;
        }
    }
    unregistered
}

/// Bir ekin türü: resimler önizlemeli görünür, diğer her şey dosya (belge)
/// olarak gider — WhatsApp'ta indirilebilir bir ek olarak.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MediaKind {
    Image,
    Document,
}

/// Yüklenmiş bir ek: mesaj kurulurken türü ve dosya adı gerekir.
pub struct Uploaded {
    pub up: wa_rs::upload::UploadResponse,
    pub mime: &'static str,
    /// Alıcıda görünen dosya adı (uzantısıyla). WhatsApp dosyayı bunsuz açmaz.
    pub file_name: String,
    pub kind: MediaKind,
}

/// Ekleri bir kez yükler (kişi başına tekrar yüklenmez).
pub async fn upload_attachments<F: Fn(String) + Send>(
    client: &Arc<wa_rs::Client>,
    paths: &[String],
    emit: &F,
) -> Result<Vec<Uploaded>, String> {
    let mut uploads = Vec::new();
    for (idx, p) in paths.iter().enumerate() {
        let kind = kind_for(p);
        emit(format!(
            "{} yükleniyor... ({}/{})",
            match kind {
                MediaKind::Image => "Resim",
                MediaKind::Document => "Dosya",
            },
            idx + 1,
            paths.len()
        ));
        let data = std::fs::read(p).map_err(|e| format!("Dosya okunamadı ({}): {}", p, e))?;
        let media = match kind {
            MediaKind::Image => wa_rs::download::MediaType::Image,
            MediaKind::Document => wa_rs::download::MediaType::Document,
        };
        let up = client
            .upload(data, media)
            .await
            .map_err(|e| format!("Dosya yüklenemedi ({}): {}", p, e))?;
        uploads.push(Uploaded {
            up,
            mime: mime_for(p),
            file_name: base_name(p),
            kind,
        });
    }
    Ok(uploads)
}

/// Bir kişiye gönderilecek mesajları kurar. Metin varsa tek metin mesajı;
/// ek varsa ilki mesaj metniyle (altyazı) birlikte, kalanlar ayrı mesaj olarak.
pub fn build_messages(
    message: &str,
    contact_name: &str,
    uploads: &[Uploaded],
    view_once: bool,
) -> Vec<wa_rs::wa_rs_proto::whatsapp::Message> {
    if uploads.is_empty() {
        return vec![build_text_message(message, contact_name)];
    }
    uploads
        .iter()
        .enumerate()
        .map(|(idx, up)| {
            let caption = if idx == 0 { message } else { "" };
            match up.kind {
                MediaKind::Image => {
                    build_image_message(caption, contact_name, up.mime, up.up.clone(), view_once)
                }
                MediaKind::Document => build_document_message(caption, contact_name, up),
            }
        })
        .collect()
}

/// Bir mesajı gönderir; geçici hatada tekrar dener, bağlantı koptuysa
/// yeniden bağlanmasını bekler. Hata mesajını döndürür.
pub async fn send_one<F: Fn(String) + Send>(
    client: &Arc<wa_rs::Client>,
    jid: &wa_rs::Jid,
    msg: &wa_rs::wa_rs_proto::whatsapp::Message,
    stop: &AtomicBool,
    emit: &F,
) -> Result<(), String> {
    let mut last_err = String::new();
    for attempt in 1..=MAX_SEND_ATTEMPTS {
        let sent = tokio::time::timeout(
            Duration::from_secs(SEND_TIMEOUT_SECS),
            client.send_message(jid.clone(), msg.clone()),
        )
        .await;
        match sent {
            Ok(Ok(_)) => return Ok(()),
            Ok(Err(e)) => last_err = e.to_string(),
            Err(_) => last_err = format!("{} sn içinde yanıt gelmedi", SEND_TIMEOUT_SECS),
        }

        if attempt < MAX_SEND_ATTEMPTS {
            emit(format!("Hata ({}). Tekrar deneniyor...", last_err));
            if !interruptible_sleep(3 + attempt as u64 * 2, stop).await {
                return Err("durduruldu".to_string());
            }
            if !(client.is_connected() && client.is_logged_in()) {
                let _ = client
                    .wait_for_connected(Duration::from_secs(CONNECT_WAIT_SECS))
                    .await;
            }
        }
    }
    Err(last_err)
}

/// Henüz gönderilmemiş kişileri "kalanlar" listesine ekler; böylece Durdur'a
/// basıldığında ya da bağlantı kopup geri gelmediğinde "tekrar dene" tam olarak
/// kalan kişileri gönderir.
pub fn push_pending(summary: &mut SendSummary, rest: &[&Contact]) {
    for c in rest {
        summary.pending.push((*c).clone());
    }
}

/// Toplu gönderim döngüsü. Tauri'den bağımsız olsun diye `emit` bir closure.
///
/// - Gönderimden önce numaraları doğrular (WhatsApp'ta olmayanları atlar).
/// - Bağlantı koparsa mesajı yakmayıp yeniden bağlanmasını bekler.
/// - Geçici hatada tekrar dener, kalıcı hatada kalanlara devam eder.
/// - Ekleri (resim/belge) bir kez yükler, "Durdur" ile bekleme sırasında da kesilebilir.
// Argümanlar tek tek geçiliyor: hepsi gönderim çağrısının doğal parçası ve bir
// seçenek struct'ı sarmalamak iki çağıran için de dolaylılık katardı.
#[allow(clippy::too_many_arguments)]
pub async fn send_batch<F: Fn(String) + Send>(
    client: Arc<wa_rs::Client>,
    contacts: &[Contact],
    message: &str,
    attachment_paths: &[String],
    validate: bool,
    view_once: bool,
    stop: &AtomicBool,
    emit: F,
) -> Result<SendSummary, String> {
    let mut summary = SendSummary {
        total: contacts.len(),
        sent: 0,
        failed: 0,
        skipped: 0,
        pending: Vec::new(),
    };

    // Not: zamanlanmış gönderim burada beklenmez. İş `scheduled` kuyruğuna
    // yazılır ve vakti gelince `scheduler_tick` bu fonksiyonu çağırır; böylece
    // bekleme sırasında gönderim hattı kilitli kalmaz.
    // 0) Bağlantı hazır değilse kısa bir süre bekle; gelmezse anlaşılır hata ver.
    if !(client.is_connected() && client.is_logged_in()) {
        emit("WhatsApp bağlantısı bekleniyor...".to_string());
        client
            .wait_for_connected(Duration::from_secs(CONNECT_WAIT_START_SECS))
            .await
            .map_err(|_| {
                "WhatsApp bağlantısı yok. Önce 'WhatsApp Bağla' ile QR kodu okutun.".to_string()
            })?;
    }

    // 1) Ekleri bir kez yükle (kişi başına tekrar yüklenmez).
    let uploads = upload_attachments(&client, attachment_paths, &emit).await?;

    // 2) Numaraları doğrula.
    let mut targets: Vec<&Contact> = contacts.iter().collect();
    if validate {
        emit("Numaralar WhatsApp'ta kayıtlı mı diye kontrol ediliyor...".to_string());
        let unregistered = find_unregistered(&client, contacts, stop, &emit).await;
        // Liste neredeyse tamamen "yok" çıktıysa doğrulama şüpheli: yok say.
        if !unregistered.is_empty()
            && unregistered.len() * 5 > contacts.len() * 4
            && contacts.len() >= 10
        {
            emit(format!(
                "Uyarı: {} numaranın WhatsApp'ta olmadığı bildirildi, oran şüpheli olduğu için doğrulama yok sayıldı.",
                unregistered.len()
            ));
        } else {
            summary.skipped = unregistered.len();
            targets = contacts
                .iter()
                .filter(|c| !unregistered.contains(&c.phone))
                .collect();
        }
    }

    emit(format!(
        "Gönderim başlıyor: {} kişi{}",
        targets.len(),
        if summary.skipped > 0 {
            format!(
                " ({} kişi WhatsApp'ta kayıtlı değil, atlandı)",
                summary.skipped
            )
        } else {
            String::new()
        }
    ));

    // 3) Gönder.
    let mut i = 0usize;
    while i < targets.len() {
        if stop.load(Ordering::SeqCst) {
            push_pending(&mut summary, &targets[i..]);
            emit(format!("Durduruldu. {} kişiye gönderildi.", summary.sent));
            break;
        }
        let contact = targets[i];

        // Bağlantı koptuysa mesajı yakmayıp geri gelmesini bekle.
        if !(client.is_connected() && client.is_logged_in()) {
            emit(format!(
                "Bağlantı koptu, yeniden bağlanması bekleniyor... ({}/{})",
                i + 1,
                targets.len()
            ));
            if client
                .wait_for_connected(Duration::from_secs(CONNECT_WAIT_SECS))
                .await
                .is_err()
            {
                emit("Bağlantı geri gelmedi, gönderim durduruldu. Kalanlar 'Kalanları gönder' ile gönderilebilir.".to_string());
                push_pending(&mut summary, &targets[i..]);
                break;
            }
            emit("Bağlantı geri geldi, devam ediliyor...".to_string());
        }

        emit(format!(
            "Gönderiliyor... ({}/{}) - {}",
            i + 1,
            targets.len(),
            contact.name
        ));

        // İlk ek mesaj metniyle birlikte, kalanlar ayrı mesaj olarak gider.
        let msgs = build_messages(message, &contact.name, &uploads, view_once);

        let jid_str = format!("{}@s.whatsapp.net", contact.phone);
        let jid = match jid_str.parse::<wa_rs::Jid>() {
            Ok(j) => j,
            Err(_) => {
                summary.failed += 1;
                summary.pending.push(contact.clone());
                emit(format!("Atlandı (geçersiz numara): {}", contact.name));
                i += 1;
                continue;
            }
        };

        let mut last_err = String::new();
        let mut ok = true;
        for (mi, msg) in msgs.iter().enumerate() {
            if let Err(e) = send_one(&client, &jid, msg, stop, &emit).await {
                ok = false;
                last_err = e;
                break;
            }
            // Aynı kişiye giden sonraki resim için kısa bir nefes.
            if mi + 1 < msgs.len() && !interruptible_sleep(2 + random_delay_secs() % 3, stop).await
            {
                ok = false;
                last_err = "durduruldu".to_string();
                break;
            }
        }

        i += 1;

        // Bu kişi gönderildi mi? (Durdurulmuşsa hata saymıyoruz, kalanlara giriyor.)
        if ok {
            summary.sent += 1;
        } else if !stop.load(Ordering::SeqCst) {
            summary.failed += 1;
            summary.pending.push(contact.clone());
            emit(format!("Gönderilemedi ({}): {}", last_err, contact.name));
        }

        if stop.load(Ordering::SeqCst) {
            if !ok {
                summary.pending.push(contact.clone());
            }
            push_pending(&mut summary, &targets[i..]);
            emit(format!("Durduruldu. {} kişiye gönderildi.", summary.sent));
            break;
        }

        // Son mesajdan sonra beklemeye gerek yok.
        if i < targets.len() {
            let secs = random_delay_secs();
            emit(format!("{} sn bekleniyor...", secs));
            if !interruptible_sleep(secs, stop).await {
                push_pending(&mut summary, &targets[i..]);
                emit(format!("Durduruldu. {} kişiye gönderildi.", summary.sent));
                break;
            }
        }

        // Uzun listelerde her 25 kişide bir mola (ban riskini azaltır).
        if i.is_multiple_of(BREAK_EVERY) && i < targets.len() {
            let mola = 15 + random_delay_secs() * 3;
            emit(format!(
                "{} kişilik blok bitti, {} sn mola...",
                BREAK_EVERY, mola
            ));
            if !interruptible_sleep(mola, stop).await {
                push_pending(&mut summary, &targets[i..]);
                emit(format!("Durduruldu. {} kişiye gönderildi.", summary.sent));
                break;
            }
        }
    }

    Ok(summary)
}

/// Ekin yolu olmadan yalnızca dosya adı (WhatsApp alıcıda bunu gösterir).
/// Vakti gelen zamanlanmış işleri gönderir (masaüstünde ve sunucuda aynı
/// fonksiyon, turu çağıran taraf kurar).
///
/// Elle bir gönderim sürüyorsa ya da WhatsApp bağlı değilse hiçbir şeye
/// dokunmaz: iş kuyrukta bekler, sonraki turda gider.
pub async fn scheduler_tick<E>(state: &AppState, emit: E)
where
    E: Fn(&str, serde_json::Value) + Clone + Send,
{
    // Sıra: aynı anda tek gönderim. Meşgulse bu turu atla (iş bekler).
    if state.busy.swap(true, Ordering::SeqCst) {
        return;
    }

    let client = { state.client.lock().unwrap().clone() };
    let client = match client {
        Some(c) => c,
        None => {
            state.busy.store(false, Ordering::SeqCst);
            return;
        }
    };

    let due = {
        let db = state.db.lock().unwrap();
        take_due_scheduled(&db, now_unix()).unwrap_or_default()
    };

    for job in &due {
        state.stop.store(false, Ordering::SeqCst);
        emit(
            "send_status",
            serde_json::Value::String(format!(
                "⏰ Zamanlanmış gönderim başladı: {} kişi",
                job.contacts.len()
            )),
        );

        let batch_emit = {
            let emit = emit.clone();
            move |msg: String| emit("send_status", serde_json::Value::String(msg))
        };

        let result = send_batch(
            client.clone(),
            &job.contacts,
            &job.message,
            &job.attachments,
            job.validate,
            job.view_once,
            &state.stop,
            batch_emit,
        )
        .await;

        let (status, detail) = match &result {
            Ok(s) if s.failed == 0 && s.pending.is_empty() => {
                ("sent", format!("{}/{} gönderildi", s.sent, s.total))
            }
            Ok(s) => (
                "failed",
                format!(
                    "{}/{} gönderildi, {} kişi kaldı",
                    s.sent,
                    s.total,
                    s.pending.len() + s.failed
                ),
            ),
            Err(e) => ("failed", e.clone()),
        };

        {
            let db = state.db.lock().unwrap();
            let _ = finish_scheduled(&db, job.id, status, &detail);
        }
        emit("scheduled_changed", serde_json::Value::Null);
    }

    state.busy.store(false, Ordering::SeqCst);
}

pub fn base_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .trim()
        .to_string()
}

/// WhatsApp'ta önizlemeli (resim mesajı) giden uzantılar. Geri kalan her şey
/// belge olarak gider — `gif` dahil: WhatsApp gif'i resim mesajı olarak
/// göstermiyor, dosya olarak göndermek daha dürüst.
const IMAGE_EXTS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// Yolun uzantısına göre resim mi belge mi olduğuna karar verir. Karar burada
/// verilir (arayüzde değil): sunucu modunda seçilen dosya sunucuya yüklenir ve
/// uzantı korunur, ayrıca iki arayüz aynı davranışı gösterir.
pub fn kind_for(path: &str) -> MediaKind {
    let name = base_name(path).to_lowercase();
    let ext = name.rsplit('.').next().unwrap_or("");
    if IMAGE_EXTS.contains(&ext) {
        MediaKind::Image
    } else {
        MediaKind::Document
    }
}

pub fn mime_for(path: &str) -> &'static str {
    let name = base_name(path).to_lowercase();
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "zip" => "application/zip",
        "rar" => "application/vnd.rar",
        "7z" => "application/x-7z-compressed",
        // mp3/mp4 ayrı media mesajlarıyla gönderilmeli; burada belge olarak gider.
        _ => "application/octet-stream",
    }
}

pub fn build_text_message(text: &str, contact_name: &str) -> wa_rs::wa_rs_proto::whatsapp::Message {
    wa_rs::wa_rs_proto::whatsapp::Message {
        extended_text_message: Some(Box::new(
            wa_rs::wa_rs_proto::whatsapp::message::ExtendedTextMessage {
                text: Some(text.replace("/isim", contact_name)),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

pub fn build_image_message(
    text: &str,
    contact_name: &str,
    mime: &str,
    up: wa_rs::upload::UploadResponse,
    view_once: bool,
) -> wa_rs::wa_rs_proto::whatsapp::Message {
    wa_rs::wa_rs_proto::whatsapp::Message {
        image_message: Some(Box::new(
            wa_rs::wa_rs_proto::whatsapp::message::ImageMessage {
                url: Some(up.url),
                mimetype: Some(mime.to_string()),
                caption: if text.is_empty() {
                    None
                } else {
                    Some(text.replace("/isim", contact_name))
                },
                file_sha256: Some(up.file_sha256),
                file_length: Some(up.file_length),
                media_key: Some(up.media_key),
                file_enc_sha256: Some(up.file_enc_sha256),
                direct_path: Some(up.direct_path),
                view_once: if view_once { Some(true) } else { None },
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

/// Belge mesajı (pdf, ofis dosyası, zip...). `file_name` olmadan WhatsApp
/// alıcıda dosyayı açamaz; `title` de aynı adı taşır, yoksa liste görünümü boş
/// kalır.
pub fn build_document_message(
    text: &str,
    contact_name: &str,
    ek: &Uploaded,
) -> wa_rs::wa_rs_proto::whatsapp::Message {
    wa_rs::wa_rs_proto::whatsapp::Message {
        document_message: Some(Box::new(
            wa_rs::wa_rs_proto::whatsapp::message::DocumentMessage {
                url: Some(ek.up.url.clone()),
                mimetype: Some(ek.mime.to_string()),
                title: Some(ek.file_name.clone()),
                file_name: Some(ek.file_name.clone()),
                file_sha256: Some(ek.up.file_sha256.clone()),
                file_length: Some(ek.up.file_length),
                media_key: Some(ek.up.media_key.clone()),
                file_enc_sha256: Some(ek.up.file_enc_sha256.clone()),
                direct_path: Some(ek.up.direct_path.clone()),
                caption: if text.is_empty() {
                    None
                } else {
                    Some(text.replace("/isim", contact_name))
                },
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

// ------------------------------------------------------------------ olaylar

/// wa-rs olaylarını arayüze taşınacak (isim, mesaj) çiftine çevirir.
/// Eski sürüm "bağlı" bilgisini neredeyse her olayda gönderdiği için arayüz
/// bağlantı kopmuş olsa bile "Bağlı (Hazır)" gösteriyordu.
pub fn map_event(event: &Event) -> Option<(&'static str, String)> {
    match event {
        Event::PairingQrCode { code, .. } => Some(("qr_code", code.clone())),
        // QR yerine telefon numarasıyla eşleştirme: 8 karakterli kodu arayüze ver.
        Event::PairingCode { code, .. } => Some(("pairing_code", code.clone())),
        Event::Connected(_) | Event::PairSuccess(_) => Some(("whatsapp_ready", String::new())),
        Event::Disconnected(_) => Some((
            "whatsapp_disconnected",
            "Bağlantı koptu, yeniden bağlanılıyor...".to_string(),
        )),
        Event::LoggedOut(_) => Some((
            "whatsapp_logged_out",
            "Oturum kapatıldı. Lütfen QR kodu tekrar okutun.".to_string(),
        )),
        Event::TemporaryBan(b) => Some((
            "whatsapp_error",
            format!("GEÇİCİ BAN: {}", ban_reason_tr(&b.code)),
        )),
        Event::ConnectFailure(f) => {
            Some(("whatsapp_error", format!("Bağlantı hatası: {}", f.message)))
        }
        Event::StreamReplaced(_) => Some((
            "whatsapp_error",
            "Oturum başka bir cihazdan açıldı.".to_string(),
        )),
        Event::StreamError(e) => Some(("whatsapp_error", format!("Akış hatası: {}", e.code))),
        _ => None,
    }
}

pub fn ban_reason_tr(reason: &wa_rs::types::events::TempBanReason) -> &'static str {
    use wa_rs::types::events::TempBanReason as R;
    match reason {
        R::SentToTooManyPeople => "çok fazla kişiye kısa sürede mesaj gönderildi",
        R::BlockedByUsers => "çok fazla kişi tarafından engellendiniz",
        R::CreatedTooManyGroups => "çok fazla grup oluşturuldu",
        R::SentTooManySameMessage => "aynı mesaj çok fazla kez gönderildi",
        R::BroadcastList => "yayın listesi kullanıldı",
        R::Unknown(_) => "bilinmeyen sebep",
    }
}

// ------------------------------------------------------------------ veritabanı

pub fn init_db(app_dir: &std::path::Path) -> Result<Connection, String> {
    let db_path = app_dir.join("contacts.sqlite");
    let db = Connection::open(db_path).map_err(|e| format!("Veritabanı açılamadı: {}", e))?;
    db.execute(
        "CREATE TABLE IF NOT EXISTS contacts (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, phone TEXT UNIQUE)",
        [],
    )
    .map_err(|e| e.to_string())?;
    db.execute(
        "CREATE TABLE IF NOT EXISTS groups (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT UNIQUE, contact_ids TEXT)",
        [],
    )
    .map_err(|e| e.to_string())?;
    db.execute(
        "CREATE TABLE IF NOT EXISTS scheduled (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            send_at INTEGER NOT NULL,
            message TEXT NOT NULL DEFAULT '',
            attachments TEXT NOT NULL DEFAULT '[]',
            contacts TEXT NOT NULL DEFAULT '[]',
            validate INTEGER NOT NULL DEFAULT 1,
            view_once INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'pending',
            detail TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    // Eski kurulum: kolon sonradan eklendi (varsa hata yok sayılır).
    let _ = db.execute(
        "ALTER TABLE scheduled ADD COLUMN view_once INTEGER NOT NULL DEFAULT 0",
        [],
    );
    Ok(db)
}
// ------------------------------------------------------- komut çekirdekleri
//
// Masaüstü Tauri komutları ve sunucu HTTP uçları bu fonksiyonlara delege
// eder; mantık tek yerde durur.

pub fn get_contacts(db: &Connection) -> Result<Vec<Contact>, String> {
    let mut stmt = db
        .prepare("SELECT id, name, phone FROM contacts ORDER BY name COLLATE NOCASE ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Contact {
                id: row.get(0)?,
                name: row.get(1)?,
                phone: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn add_contact(db: &Connection, name: &str, phone: &str) -> Result<(), String> {
    let normalized =
        normalize_phone(phone).ok_or_else(|| format!("Geçersiz telefon numarası: {}", phone))?;
    let name = if name.trim().is_empty() {
        "İsimsiz".to_string()
    } else {
        name.trim().to_string()
    };
    let changed = db
        .execute(
            "INSERT OR IGNORE INTO contacts (name, phone) VALUES (?1, ?2)",
            params![name, normalized],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err(format!("Bu numara zaten kayıtlı: {}", normalized));
    }
    Ok(())
}

pub fn remove_contact(db: &Connection, phone: &str) -> Result<(), String> {
    db.execute("DELETE FROM contacts WHERE phone = ?1", [phone])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Bütün kişileri (ve onlara bağlı grupları) siler; silinen kişi sayısını döndürür.
pub fn clear_contacts(db: &Connection) -> Result<usize, String> {
    let deleted = db
        .execute("DELETE FROM contacts", [])
        .map_err(|e| e.to_string())?;
    db.execute("DELETE FROM groups", [])
        .map_err(|e| e.to_string())?;
    Ok(deleted)
}

/// vCard/CSV içeriğini çözümleyip kişileri ekler. Uzantıya göre ayrıştırıcı seçer.
pub fn import_contacts_bytes(
    db: &mut Connection,
    filename: &str,
    bytes: &[u8],
) -> Result<ImportResult, String> {
    let content = String::from_utf8_lossy(bytes);
    let pairs = if filename.to_lowercase().ends_with(".csv") {
        parse_csv(&content)
    } else {
        parse_vcf(&content)
    };
    if pairs.is_empty() {
        return Err("Dosyada geçerli bir isim/telefon bulunamadı.".to_string());
    }
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let result = insert_contacts(&tx, &pairs).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(result)
}

pub fn import_contacts_from_file(db: &mut Connection, path: &Path) -> Result<ImportResult, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Dosya okunamadı: {}", e))?;
    let filename = path.to_string_lossy().to_string();
    import_contacts_bytes(db, &filename, &bytes)
}

pub fn save_group(db: &Connection, name: &str, contact_ids: &[i64]) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Grup adı boş olamaz.".to_string());
    }
    let ids_str = contact_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<String>>()
        .join(",");
    db.execute(
        "INSERT OR REPLACE INTO groups (name, contact_ids) VALUES (?1, ?2)",
        params![name, ids_str],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn get_groups(db: &Connection) -> Result<Vec<Group>, String> {
    let mut stmt = db
        .prepare("SELECT name, contact_ids FROM groups ORDER BY name COLLATE NOCASE")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let ids_str: String = row.get(1)?;
            let contact_ids = ids_str
                .split(',')
                .filter_map(|s| s.trim().parse::<i64>().ok())
                .collect();
            Ok(Group { name, contact_ids })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn delete_group(db: &Connection, name: &str) -> Result<(), String> {
    db.execute("DELETE FROM groups WHERE name = ?1", [name])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ------------------------------------------------------------ zamanlanmış
//
// Gmail'deki "zamanlanmış mesajlar" gibi: iş kuyruğa yazılır, gönderim ayrı bir
// arka plan turunda (`scheduler_tick`) yapılır. Böylece "zamanla" demek gönderim
// hattını kilitlemez; iş beklerken arayüz başka gönderimleri kabul eder, vakti
// gelince kendiliğinden gider ve listeden iptal edilebilir.
//
// ==== SCHEDULE BEGIN ====
// Bu blok wa-rs ve tauri'ye dokunmaz: sandbox'ta yalnızca rusqlite ile derlenip
// test edilebilsin diye ayrı tutulmuştur (bkz. AGENTS.md doğrulama akışı).

/// Kuyrukta bekleyen bir zamanlanmış gönderim.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ScheduledJob {
    pub id: i64,
    /// UNIX saniyesi (arayüzde yerel saat seçilir).
    pub send_at: u64,
    pub message: String,
    pub attachments: Vec<String>,
    pub contacts: Vec<Contact>,
    pub validate: bool,
    /// Görseller tek görüntülük (view once) gönderilsin mi.
    pub view_once: bool,
    /// pending | sending | sent | failed | missed
    pub status: String,
    /// İnsan okuyabilir sonuç: "12/12 gönderildi" gibi.
    pub detail: String,
    pub created_at: u64,
}

/// Arka plan turunun sıklığı (saniye).
pub const SCHEDULE_POLL_SECS: u64 = 15;
/// Vakti bu kadar geçen iş artık kendiliğinden gönderilmez: "kaçırıldı" olur.
pub const SCHEDULE_MISSED_GRACE_SECS: u64 = 2 * 3600;

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledJob> {
    let attachments: String = row.get(3)?;
    let contacts: String = row.get(4)?;
    Ok(ScheduledJob {
        id: row.get(0)?,
        // SQLite INTEGER = i64; epoch saniyesi rahat sığar (rusqlite u64 desteklemez).
        send_at: row.get::<_, i64>(1)? as u64,
        message: row.get(2)?,
        attachments: serde_json::from_str(&attachments).unwrap_or_default(),
        contacts: serde_json::from_str(&contacts).unwrap_or_default(),
        validate: row.get(5)?,
        view_once: row.get::<_, i64>(9)? != 0,
        status: row.get(6)?,
        detail: row.get(7)?,
        created_at: row.get::<_, i64>(8)? as u64,
    })
}

/// İşi kuyruğa ekler; yeni kaydın id'sini döndürür.
pub fn add_scheduled(
    db: &Connection,
    send_at: u64,
    message: &str,
    attachments: &[String],
    contacts: &[Contact],
    validate: bool,
    view_once: bool,
) -> Result<i64, String> {
    if contacts.is_empty() {
        return Err("Zamanlanacak kişi seçilmedi.".to_string());
    }
    if message.trim().is_empty() && attachments.is_empty() {
        return Err("Mesaj da ek de yok.".to_string());
    }
    if send_at <= now_unix() {
        return Err("Geçmiş bir saat seçilemez.".to_string());
    }
    let attachments =
        serde_json::to_string(attachments).map_err(|e| format!("Ekler yazılamadı: {}", e))?;
    let contacts =
        serde_json::to_string(contacts).map_err(|e| format!("Kişiler yazılamadı: {}", e))?;
    db.execute(
        "INSERT INTO scheduled (send_at, message, attachments, contacts, validate, view_once, status, detail, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', '', ?7)",
        params![
            send_at as i64,
            message,
            attachments,
            contacts,
            validate,
            view_once,
            now_unix() as i64
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(db.last_insert_rowid())
}

/// Kuyruğu listeler: bekleyenler (vakte göre) önce, bitenler sonra.
pub fn list_scheduled(db: &Connection) -> Result<Vec<ScheduledJob>, String> {
    let mut stmt = db
        .prepare(
            "SELECT id, send_at, message, attachments, contacts, validate, status, detail, created_at, view_once
             FROM scheduled
             ORDER BY CASE WHEN status = 'pending' THEN 0 ELSE 1 END ASC,
                      CASE WHEN status = 'pending' THEN send_at ELSE -send_at END ASC
             LIMIT 200",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], row_to_job).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// Henüz başlamamış bir işi kuyruktan siler. Başlamış/bitmiş iş silinmez.
pub fn cancel_scheduled(db: &Connection, id: i64) -> Result<(), String> {
    let n = db
        .execute(
            "DELETE FROM scheduled WHERE id = ?1 AND status = 'pending'",
            params![id],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("Bu iş artık iptal edilemez (başlamış ya da bitmiş).".to_string());
    }
    Ok(())
}

/// Bitmiş işleri listeden temizler (bekleyenlere dokunmaz).
pub fn clear_finished_scheduled(db: &Connection) -> Result<usize, String> {
    db.execute("DELETE FROM scheduled WHERE status <> 'pending'", [])
        .map_err(|e| e.to_string())
}

/// "Şimdi gönder": işi yeniden kuyruğa alır (kaçırılmış/hatalı işler için).
pub fn send_scheduled_now(db: &Connection, id: i64) -> Result<(), String> {
    // Durum ayrı sorulur: 0 satır güncellenmenin iki farklı nedeni var, ikisine
    // de aynı mesajı vermek kullanıcıyı yanıltıyor.
    let durum: Option<String> = db
        .query_row(
            "SELECT status FROM scheduled WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    match durum.as_deref() {
        None => return Err("Bu iş bulunamadı.".to_string()),
        Some("sending") => return Err("Bu iş şu anda gönderiliyor.".to_string()),
        _ => {}
    }
    db.execute(
        "UPDATE scheduled SET status = 'pending', send_at = ?2, detail = '' WHERE id = ?1",
        params![id, now_unix() as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Vakti gelen işleri `sending` olarak ayırıp döndürür; vakti çoktan geçenleri
/// `missed` yapar (uygulama o sırada kapalıydı).
///
/// `sending` durumunda kalmış satırlar (gönderim ortasında kapanma) yeniden
/// kuyruğa alınır: tur yalnızca boşta çalıştığı için bu güvenlidir.
pub fn take_due_scheduled(db: &Connection, now: u64) -> Result<Vec<ScheduledJob>, String> {
    db.execute(
        "UPDATE scheduled SET status = 'pending', detail = '' WHERE status = 'sending'",
        [],
    )
    .map_err(|e| e.to_string())?;

    let mut stmt = db
        .prepare(
            "SELECT id, send_at, message, attachments, contacts, validate, status, detail, created_at, view_once
             FROM scheduled WHERE status = 'pending' AND send_at <= ?1 ORDER BY send_at ASC",
        )
        .map_err(|e| e.to_string())?;
    // Önce topla: aynı bağlantıda açık bir SELECT varken UPDATE çalıştırmak
    // SQLite'ta "table is locked" hatası verebiliyor.
    let jobs = stmt
        .query_map(params![now as i64], row_to_job)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);

    let mut due = Vec::new();
    for job in jobs {
        if now.saturating_sub(job.send_at) > SCHEDULE_MISSED_GRACE_SECS {
            db.execute(
                "UPDATE scheduled SET status = 'missed', detail = ?2 WHERE id = ?1",
                params![
                    job.id,
                    "Vakti geçmiş (uygulama kapalıydı) — 'Şimdi gönder' ile yollayabilirsiniz."
                ],
            )
            .map_err(|e| e.to_string())?;
        } else {
            db.execute(
                "UPDATE scheduled SET status = 'sending', detail = 'Gönderiliyor' WHERE id = ?1",
                params![job.id],
            )
            .map_err(|e| e.to_string())?;
            due.push(job);
        }
    }
    Ok(due)
}

/// İşin sonucunu yazar: `sent` | `failed` | `missed`.
pub fn finish_scheduled(
    db: &Connection,
    id: i64,
    status: &str,
    detail: &str,
) -> Result<(), String> {
    db.execute(
        "UPDATE scheduled SET status = ?2, detail = ?3 WHERE id = ?1",
        params![id, status, detail],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ==== SCHEDULE END ====

// ------------------------------------------------------------- WhatsApp

/// Arayüzün açılışta doğru durumu göstermesi için.
pub fn wa_status(client: &Option<Arc<wa_rs::Client>>) -> String {
    match client.as_ref() {
        None => "offline".to_string(),
        Some(c) if c.is_connected() && c.is_logged_in() => "connected".to_string(),
        Some(_) => "connecting".to_string(),
    }
}

/// WhatsApp istemcisini kurar, `on_client` ile dışarı verir ve çalıştırır.
///
/// wa-rs kopan bağlantıyı kendi içinde (artan beklemeyle) yeniden kurar;
/// bizim işimiz istemciyi ayakta tutmak ve olayları `emit` ile bildirmek.
/// Olay adları/yükleri her iki arayüzde de aynıdır (`map_event`).
pub async fn run_bot<E, F>(
    app_dir: &Path,
    emit: E,
    on_client: F,
    stop_slot: Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
) -> Result<(), String>
where
    E: Fn(&str, serde_json::Value) + Clone + Send + Sync + 'static,
    F: FnOnce(Arc<wa_rs::Client>) + Send + 'static,
{
    std::fs::create_dir_all(app_dir).map_err(|e| e.to_string())?;
    let store_path = app_dir.join("wa_bot.db");

    // BOZUK_OTURUM: 0 baytlık dosya wa-rs'i kilitler ("cihaz bağlanamadı").
    // Açılışta böyle bir kalıntı varsa temizle, yeni kimlik üretilsin.
    if let Ok(meta) = std::fs::metadata(&store_path) {
        if meta.len() == 0 {
            let _ = std::fs::remove_file(&store_path);
            for ek in ["-wal", "-shm"] {
                let _ = std::fs::remove_file(app_dir.join(format!("wa_bot.db{}", ek)));
            }
        }
    }
    let backend = Arc::new(
        SqliteStore::new(&store_path.to_string_lossy())
            .await
            .map_err(|e| format!("WhatsApp oturumu açılamadı: {}", e))?,
    );

    let event_emit = emit.clone();
    let mut bot = Bot::builder()
        .with_backend(backend)
        .with_transport_factory(TokioWebSocketTransportFactory::new())
        .with_http_client(UreqHttpClient::new())
        .on_event(move |event, _client| {
            let emit = event_emit.clone();
            async move {
                if let Some((name, payload)) = map_event(&event) {
                    emit(name, serde_json::Value::String(payload));
                }
            }
        })
        .build()
        .await
        .map_err(|e| format!("WhatsApp istemcisi başlatılamadı: {}", e))?;

    on_client(bot.client());
    // `run()` yeni bir görev açar; onu öldürebilmek için handle'ı dışarı ver.
    let wa_handle = bot
        .run()
        .await
        .map_err(|e| format!("WhatsApp bağlantısı kurulamadı: {}", e))?;
    // Çalışma süresi handle'ı dışarı verilir: "Kes"/"Çıkış" bu handle'ı abort
    // eder. `bot` düşürülse bile süren `run()` döngüsü kendiliğinden ölmez ve
    // oturum yeniden bağlanırdı; çıkış bu yüzden etkisiz kalıyordu.
    *stop_slot.lock().unwrap() = Some(wa_handle);
    emit("whatsapp_connecting", serde_json::Value::Null);
    // Görev kendi içinde sürer; bağlantı bittiğinde await'in dönmesi için
    // sonsuza kadar beklenir (abort edilirse bu future düşer).
    std::future::pending::<()>().await;
    Ok(())
}

/// Sadece bağlantıyı keser; oturum korunur, bir sonraki açılışta QR gerekmez.
///
/// Tek başına `client.disconnect()` yetmiyordu: istemci kendi kendine yeniden
/// bağlanıyor (auto-reconnect) ve çağrı, süren bir bağlanma denemesiyle
/// yarışırsa etkisiz kalıyordu — kullanıcı "Kes"e basıyor, WhatsApp bağlı
/// kalıyordu. Burada otomatik yeniden bağlanma kapatılır, bağlantı gerçekten
/// düşene kadar kısa bir döngüyle tekrar denenir ve istemci düşürülür.
pub async fn disconnect_session(state: &AppState) -> Result<(), String> {
    // Önce eldeki istemci düşürülür (kilidi tutarken await yok).
    let client = {
        let mut slot = state.client.lock().unwrap();
        slot.take()
    };
    // Bu tur artık eski: kapanışı yeni bağlantıya dokunmasın.
    if let Some(token) = state.running.lock().unwrap().as_ref() {
        token.store(false, Ordering::SeqCst);
    }
    // Sürüp giden `bot.run()` döngüsünü öldür: istemci nesnesini düşürmek
    // yetmiyordu, döngü kendi kendine yeniden bağlanıyordu. Abort'un etkili
    // olması beklenir — aksi hâlde çağrı dönerken döngü hâlâ soket tutuyor
    // olabilir ve yerel oturum dosyası kilitli kalır.
    let handle = { state.stop_handle.lock().unwrap().take() };
    if let Some(handle) = handle {
        handle.abort();
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    }

    // `client` None olsa bile bağlanma/QR turu arka planda sürebiliyor
    // (spawn_connect). Eskiden burada "Zaten bağlı değil." dönülüyordu ve
    // kullanıcı çıkış yaptığını sanırken tur yeniden bağlanıyordu — bu yüzden
    // işlem her durumda sürer.
    match client {
        Some(client) => {
            // Süren gönderim varsa dursun; yoksa bağlantı kesilse bile mesaj akmaya devam eder.
            state.stop.store(true, Ordering::SeqCst);
            client.enable_auto_reconnect.store(false, Ordering::Relaxed);
            for _ in 0..10 {
                client.disconnect().await;
                if !client.is_connected() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        }
        None => {
            // İstemci hiç oluşmadıysa yalnızca sürüp giden bağlanma turu durdurulur.
            state.stop.store(true, Ordering::SeqCst);
        }
    }

    // Arayüz ve gönderim yolu artık "bağlı" görmesin.
    *state.client.lock().unwrap() = None;
    Ok(())
}

/// QR yerine telefon numarasıyla eşleştirme başlatır.
///
/// Dönen 8 karakterli kod telefonda
/// "Bağlı cihazlar → Cihaz bağla → Telefon numarasıyla bağlan" ekranına girilir.
/// QR ile aynı anda çalışabilir; hangisi önce tamamlanırsa o kazanır.
pub async fn pair_with_phone(
    state: &AppState,
    phone: &str,
) -> Result<String, String> {
    // Numara: yalnızca rakam, ülke kodu dahil (ör. 905551234567)
    let digits: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 10 || digits.len() > 15 {
        return Err(format!(
            "Geçersiz numara: {} hane. Ülke koduyla, yalnızca rakam girin (ör. 905551234567).",
            digits.len()
        ));
    }

    let client = state
        .client
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Önce bağlantı başlatılmalı (connect_whatsapp).".to_string())?;

    client
        .pair_with_code(wa_rs::pair_code::PairCodeOptions {
            phone_number: digits,
            ..Default::default()
        })
        .await
        .map_err(|e| format!("Eşleştirme kodu alınamadı: {}", e))
}

/// Çıkış: bağlantıyı keser ve yerel oturumu siler.
///
/// Bağlı değilken de çalışır — oturum dosyası kalırsa uygulama bir sonraki
/// açılışta yine kendiliğinden bağlanmaya çalışırdı.
pub async fn logout_session(state: &AppState, app_dir: &Path) -> Result<(), String> {
    disconnect_session(state).await?;
    wait_client_stopped(state).await;
    // Oturum dosyası silinemezse çıkış YİNE de başarılı sayılır: kullanıcı için
    // önemli olan bağlantının gitmesi. Dosya boşaltıldıysa eski oturum zaten
    // kullanılamaz; hata yalnızca günlüğe düşer.
    match delete_wa_session(app_dir) {
        Ok(()) => Ok(()),
        Err(e) => {
            log_line(app_dir, &format!("çıkış: dosya temizliği başarısız: {}", e));
            Ok(())
        }
    }
}

/// İstemci tamamen durana kadar bekler (oturum dosyası açıkken silinemez).
pub async fn wait_client_stopped(state: &AppState) {
    // Çıkışı kullanıcı bekliyor: toplam bekleme ~2 sn (eski 15 sn arayüzde
    // "çıkmıyor" gibi görünüyordu).
    for _ in 0..20 {
        if !state.connecting.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Yerel WhatsApp oturumunu siler (çıkış). wa-rs 0.2.0 sunucu tarafında cihazı
/// kaldıran bir API sunmuyor: bağlantı kesilir ve yerel oturum dosyaları silinir,
/// böylece bir sonraki bağlantıda QR yeniden okutulur. Telefondaki "Bağlı
/// cihazlar" kaydını kullanıcı kaldırır.
///
/// Dosya silinemezse (başka kopya/DB kilidi, izin, TCC) çıkış sessizce yarım
/// kalmamalı: oturum geçersiz sayılıp boş bir kopyayla değiştirilir. Aksi hâlde
/// eski oturum bir sonraki açılışta yine kendiliğinden bağlanıyor ve kullanıcı
/// "çıkış yaptım ama WhatsApp bağlı" durumunda kalıyordu.
pub fn delete_wa_session(app_dir: &Path) -> Result<(), String> {
    let mut hata = None;
    for name in ["wa_bot.db", "wa_bot.db-wal", "wa_bot.db-shm"] {
        let path = app_dir.join(name);
        if !path.exists() {
            continue;
        }
        if let Err(e) = std::fs::remove_file(&path) {
            hata = Some(format!("{}: {}", name, e));
        }
    }

    // Silinemeyen dosyayı SIFIRLAMA (0 baytlık wa_bot.db wa-rs'i kilitliyor ve
    // "cihaz bağlanamadı" hatasına yol açıyordu). Bunun yerine adını değiştir:
    // wa-rs yeni bir dosya açar, eski kalıntı bir sonraki açılışta silinir.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for name in ["wa_bot.db", "wa_bot.db-wal", "wa_bot.db-shm"] {
        let path = app_dir.join(name);
        if !path.exists() {
            continue;
        }
        let yedek = app_dir.join(format!("{}.eski-{}", name, stamp));
        if std::fs::rename(&path, &yedek).is_err() {
            // Ad değiştirme de olmuyorsa en azından boşaltmayı dene.
            let _ = std::fs::write(&path, b"");
        }
    }

    // Önceki çalıştırmalardan kalan .eski-* artıkları temizle (en fazla 5 tut).
    if let Ok(entries) = std::fs::read_dir(app_dir) {
        let mut eskiler: Vec<_> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains(".eski-"))
                    .unwrap_or(false)
            })
            .collect();
        eskiler.sort();
        while eskiler.len() > 5 {
            if let Some(p) = eskiler.first().cloned() {
                let _ = std::fs::remove_file(p);
                eskiler.remove(0);
            } else {
                break;
            }
        }
    }

    let _ = hata; // eski hata mesajı artık kullanılmıyor
    Ok(())
}

/// Çıkış sonrası durumu insan okur biçimde özetler (arayüzde gösterilir).
pub fn logout_report(state: &AppState, app_dir: &Path) -> String {
    let bagli = state
        .client
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.is_connected() || c.is_logged_in())
        .unwrap_or(false);
    let mut kalan = Vec::new();
    for name in ["wa_bot.db", "wa_bot.db-wal", "wa_bot.db-shm"] {
        let p = app_dir.join(name);
        if let Ok(m) = std::fs::metadata(&p) {
            kalan.push(format!("{} ({} bayt)", name, m.len()));
        }
    }
    format!(
        "bağlantı: {} · oturum dosyası: {}",
        if bagli { "HÂLÂ BAĞLI" } else { "kesildi" },
        if kalan.is_empty() {
            "silindi".to_string()
        } else {
            kalan.join(", ")
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ek(file_name: &str, mime: &'static str, kind: MediaKind) -> Uploaded {
        Uploaded {
            up: wa_rs::upload::UploadResponse {
                url: "https://mmg.whatsapp.net/v/x".to_string(),
                direct_path: "/v/x".to_string(),
                media_key: vec![1, 2, 3],
                file_sha256: vec![4],
                file_enc_sha256: vec![5],
                file_length: 42,
            },
            mime,
            file_name: file_name.to_string(),
            kind,
        }
    }

    #[test]
    fn cikis_dosya_silinemese_de_oturumu_gecersiz_kilar() {
        let dir = std::env::temp_dir().join(format!("wa_logout_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("wa_bot.db");
        std::fs::write(&db, b"eski-oturum-icerigi").unwrap();
        std::fs::write(dir.join("wa_bot.db-wal"), b"wal").unwrap();

        delete_wa_session(&dir).unwrap();

        // Dosyalar ya silinir ya sıfırlanır: her iki durumda da eski oturum yok.
        for name in ["wa_bot.db", "wa_bot.db-wal", "wa_bot.db-shm"] {
            let p = dir.join(name);
            if p.exists() {
                assert_eq!(
                    std::fs::metadata(&p).unwrap().len(),
                    0,
                    "{} boş değil",
                    name
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ek_turu_ve_mime_uzantiya_gore() {
        assert_eq!(kind_for("/tmp/foto.JPG"), MediaKind::Image);
        assert_eq!(mime_for("/tmp/foto.JPG"), "image/jpeg");
        assert_eq!(kind_for(r"C:\Documents\rapor.pdf"), MediaKind::Document);
        assert_eq!(mime_for("rapor.pdf"), "application/pdf");
        assert_eq!(
            mime_for("tablo.xlsx"),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
        // Uzantısız/tanınmayan dosya belge olarak gider, resim sanılmaz.
        assert_eq!(kind_for("/tmp/uzantisiz"), MediaKind::Document);
        assert_eq!(mime_for("/tmp/arsiv.7z"), "application/x-7z-compressed");
        // gif resim mesajı olarak gösterilmiyor -> belge olarak gider.
        assert_eq!(kind_for("anim.gif"), MediaKind::Document);
        assert_eq!(mime_for("anim.gif"), "image/gif");
    }

    #[test]
    fn dosya_adi_yoldan_ayiklanir() {
        assert_eq!(base_name("/tmp/a b/rapor.pdf"), "rapor.pdf");
        assert_eq!(base_name(r"C:\Users\x\belge.docx"), "belge.docx");
        assert_eq!(base_name("tek_dosya.pdf"), "tek_dosya.pdf");
    }

    #[test]
    fn belge_mesaji_dosya_adini_ve_altyaziyi_tasir() {
        let m = build_document_message(
            "Merhaba /isim",
            "Ahmet",
            &ek("rapor.pdf", "application/pdf", MediaKind::Document),
        );
        assert!(m.image_message.is_none());
        let d = m.document_message.expect("belge mesajı kurulmalı");
        assert_eq!(d.file_name.as_deref(), Some("rapor.pdf"));
        assert_eq!(d.title.as_deref(), Some("rapor.pdf"));
        assert_eq!(d.mimetype.as_deref(), Some("application/pdf"));
        assert_eq!(d.caption.as_deref(), Some("Merhaba Ahmet"));
        assert_eq!(d.file_length, Some(42));
        assert_eq!(d.direct_path.as_deref(), Some("/v/x"));
        assert_eq!(d.media_key.as_deref(), Some(&[1u8, 2, 3][..]));
        assert_eq!(d.file_enc_sha256.as_deref(), Some(&[5u8][..]));
    }

    #[test]
    fn altyazisiz_belgede_caption_bos_olmali() {
        let m = build_document_message(
            "",
            "Ahmet",
            &ek("sessiz.pdf", "application/pdf", MediaKind::Document),
        );
        assert_eq!(
            m.document_message.expect("belge mesajı").caption,
            None,
            "caption None olmalı, Some(\"\") değil"
        );
    }

    #[test]
    fn karisik_eklerde_ilk_ek_altyaziyi_alir() {
        let ekler = vec![
            ek("foto.jpg", "image/jpeg", MediaKind::Image),
            ek("rapor.pdf", "application/pdf", MediaKind::Document),
            ek("arsiv.zip", "application/zip", MediaKind::Document),
        ];
        let msgs = build_messages("Merhaba /isim", "Ayşe", &ekler, false);
        assert_eq!(msgs.len(), 3);
        let img = msgs[0].image_message.as_ref().expect("ilk ek resim");
        assert_eq!(img.caption.as_deref(), Some("Merhaba Ayşe"));
        let doc = msgs[1].document_message.as_ref().expect("ikinci ek belge");
        assert_eq!(doc.file_name.as_deref(), Some("rapor.pdf"));
        assert_eq!(doc.caption, None, "sonraki ekler altyazı taşımamalı");
        let zip = msgs[2].document_message.as_ref().expect("üçüncü ek belge");
        assert_eq!(zip.file_name.as_deref(), Some("arsiv.zip"));
    }

    #[test]
    fn ilk_ek_belge_ise_metin_ona_altyazi_olur() {
        let ekler = vec![ek("rapor.pdf", "application/pdf", MediaKind::Document)];
        let msgs = build_messages("Ektedir /isim", "Ali", &ekler, false);
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0]
                .document_message
                .as_ref()
                .unwrap()
                .caption
                .as_deref(),
            Some("Ektedir Ali")
        );
    }

    #[test]
    fn eksiz_mesaj_metin_mesaji_olarak_kalir() {
        let msgs = build_messages("Selam /isim", "Zeynep", &[], false);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].document_message.is_none());
        assert_eq!(
            msgs[0]
                .extended_text_message
                .as_ref()
                .unwrap()
                .text
                .as_deref(),
            Some("Selam Zeynep")
        );
    }
    // ------------------------------------------------------- zamanlanmış

    static SAYAC: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Her test kendi geçici klasöründe çalışır (paylaşılan dosya yarışı olmasın).
    fn gecici_db() -> (std::path::PathBuf, Connection) {
        let n = SAYAC.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("wa-sched-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).expect("geçici klasör");
        let db = init_db(&dir).expect("veritabanı");
        (dir, db)
    }

    fn kisi(phone: &str) -> Contact {
        Contact {
            id: 1,
            name: "Test".to_string(),
            phone: phone.to_string(),
        }
    }

    #[test]
    fn zamanlanan_is_kuyruga_yazilir() {
        let (dir, db) = gecici_db();
        let ekler = vec!["/tmp/foto.jpg".to_string()];
        let id = add_scheduled(
            &db,
            now_unix() + 600,
            "Merhaba",
            &ekler,
            &[kisi("905551112233")],
            true,
            false,
        )
        .expect("kuyruğa yazılmalı");
        assert!(id > 0);

        let liste = list_scheduled(&db).expect("liste");
        assert_eq!(liste.len(), 1);
        assert_eq!(liste[0].status, "pending");
        assert_eq!(liste[0].message, "Merhaba");
        assert_eq!(liste[0].attachments, ekler, "ekler geri okunmalı");
        assert_eq!(liste[0].contacts.len(), 1);
        assert!(liste[0].validate);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tek_goruntuluk_resim_view_once_ile_kurulur() {
        let ekler = vec![ek("foto.jpg", "image/jpeg", MediaKind::Image)];
        let normal = build_messages("", "Ali", &ekler, false);
        assert_eq!(
            normal[0].image_message.as_ref().unwrap().view_once,
            None,
            "normal gönderimde bayrak olmamalı"
        );

        let tek = build_messages("", "Ali", &ekler, true);
        assert_eq!(
            tek[0].image_message.as_ref().unwrap().view_once,
            Some(true),
            "tek görüntülük bayrağı kurulmalı"
        );
    }

    #[test]
    fn gecmis_saat_ve_bos_is_reddedilir() {
        let (dir, db) = gecici_db();
        let kisiler = vec![kisi("905551112233")];
        assert!(add_scheduled(&db, now_unix() - 60, "x", &[], &kisiler, true, false).is_err());
        assert!(add_scheduled(&db, now_unix() + 60, "x", &[], &[], true, false).is_err());
        assert!(add_scheduled(&db, now_unix() + 60, "  ", &[], &kisiler, true, false).is_err());
        assert!(list_scheduled(&db).expect("liste").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn vakti_gelen_is_bir_kez_alinir() {
        let (dir, db) = gecici_db();
        let id = add_scheduled(
            &db,
            now_unix() + 100,
            "Merhaba",
            &[],
            &[kisi("905551112233")],
            true,
            false,
        )
        .unwrap();

        assert!(
            take_due_scheduled(&db, now_unix()).expect("tur").is_empty(),
            "vakti gelmeden alınmamalı"
        );

        let alinan = take_due_scheduled(&db, now_unix() + 200).expect("tur");
        assert_eq!(alinan.len(), 1);
        assert_eq!(alinan[0].id, id);
        assert_eq!(
            list_scheduled(&db).unwrap()[0].status,
            "sending",
            "iş gönderiliyor olarak işaretlenmeli"
        );

        finish_scheduled(&db, id, "sent", "1/1 gönderildi").unwrap();
        assert!(
            take_due_scheduled(&db, now_unix() + 300)
                .expect("tur")
                .is_empty(),
            "bitmiş iş tekrar gönderilmemeli"
        );
        let bitti = &list_scheduled(&db).unwrap()[0];
        assert_eq!(bitti.status, "sent");
        assert_eq!(bitti.detail, "1/1 gönderildi");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn gonderim_ortasinda_kapanan_is_yeniden_kuyruga_girer() {
        let (dir, db) = gecici_db();
        add_scheduled(
            &db,
            now_unix() + 10,
            "Merhaba",
            &[],
            &[kisi("905551112233")],
            false,
            false,
        )
        .unwrap();
        assert_eq!(take_due_scheduled(&db, now_unix() + 20).unwrap().len(), 1);
        // Uygulama gönderim sırasında kapandı: satır `sending` kaldı.
        // Sonraki tur onu yeniden almalı (mesaj kaybolmasın).
        let tekrar = take_due_scheduled(&db, now_unix() + 40).unwrap();
        assert_eq!(tekrar.len(), 1, "yarım kalan iş yeniden kuyruğa alınmalı");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cok_geciken_is_kacirilir() {
        let (dir, db) = gecici_db();
        let id = add_scheduled(
            &db,
            now_unix() + 10,
            "Merhaba",
            &[],
            &[kisi("905551112233")],
            true,
            false,
        )
        .unwrap();
        let gec = now_unix() + SCHEDULE_MISSED_GRACE_SECS + 60;
        assert!(
            take_due_scheduled(&db, gec).unwrap().is_empty(),
            "çok geç kalan iş kendiliğinden gitmez"
        );
        let is = &list_scheduled(&db).unwrap()[0];
        assert_eq!(is.status, "missed");
        assert!(!is.detail.is_empty());

        // "Şimdi gönder" işi tekrar kuyruğa alır; olmayan iş için açık hata verir.
        assert!(send_scheduled_now(&db, id).is_ok());
        assert!(
            send_scheduled_now(&db, 4242).is_err(),
            "olmayan iş için hata beklenir"
        );
        let is = &list_scheduled(&db).unwrap()[0];
        assert_eq!(is.status, "pending");
        assert!(is.send_at <= now_unix() + 5);
        assert_eq!(take_due_scheduled(&db, now_unix() + 5).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn iptal_yalnizca_bekleyen_isi_siler() {
        let (dir, db) = gecici_db();
        let id = add_scheduled(
            &db,
            now_unix() + 300,
            "Merhaba",
            &[],
            &[kisi("905551112233")],
            true,
            false,
        )
        .unwrap();
        cancel_scheduled(&db, id).expect("bekleyen iş iptal edilmeli");
        assert!(list_scheduled(&db).unwrap().is_empty());
        assert!(
            cancel_scheduled(&db, id).is_err(),
            "ikinci iptal hata vermeli"
        );

        // Gönderilmiş iş iptal edilemez.
        let id2 = add_scheduled(
            &db,
            now_unix() + 300,
            "Merhaba",
            &[],
            &[kisi("905551112233")],
            true,
            false,
        )
        .unwrap();
        finish_scheduled(&db, id2, "sent", "1/1 gönderildi").unwrap();
        assert!(cancel_scheduled(&db, id2).is_err());

        // Geçmiş temizliği bekleyene dokunmaz.
        let id3 = add_scheduled(
            &db,
            now_unix() + 300,
            "Merhaba",
            &[],
            &[kisi("905551112233")],
            true,
            false,
        )
        .unwrap();
        assert_eq!(clear_finished_scheduled(&db).unwrap(), 1);
        let kalan = list_scheduled(&db).unwrap();
        assert_eq!(kalan.len(), 1);
        assert_eq!(kalan[0].id, id3);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn anti_ban_beklemesi_tum_degerleri_uretir() {
        let mut gorulen = std::collections::BTreeSet::new();
        for _ in 0..2000 {
            let d = random_delay_secs();
            assert!((4..=9).contains(&d), "4-9 arası olmalı, {} geldi", d);
            gorulen.insert(d);
        }
        assert_eq!(
            gorulen.len(),
            6,
            "4..9 arası tüm değerler üretilmeli, görülen: {:?}",
            gorulen
        );
    }

    #[test]
    fn token_karsilastirmasi_sabit_zamanli_calisir() {
        assert!(token_eq("abcd1234", "abcd1234"));
        assert!(!token_eq("abcd1234", "abcd1235"));
        assert!(!token_eq("abcd", "abcd1234"), "farklı uzunluk eşleşmemeli");
        assert!(token_eq("", ""));
    }
    #[test]
    fn zaman_damgasi_dogru_bicimlenir() {
        assert_eq!(fmt_utc(0), "1970-01-01 00:00:00");
        assert_eq!(fmt_utc(1_000_000_000), "2001-09-09 01:46:40");
        // 2026-10-14 15:42:07 UTC
        assert_eq!(fmt_utc(1_792_002_127), "2026-10-14 18:22:07");
    }

    #[test]
    fn gunluk_dosyasi_yazilir_ve_buyuyunce_devredilir() {
        let (dir, _db) = gecici_db();
        log_line(&dir, "deneme");
        let icerik = std::fs::read_to_string(dir.join("app.log")).expect("günlük yazılmalı");
        assert!(icerik.contains("deneme"), "mesaj günlüğe girmeli: {icerik}");
        assert!(icerik.len() > 20, "zaman damgası da yazılmalı");

        // 1 MB sınırı aşılınca app.log.1'e devredilir.
        std::fs::write(dir.join("app.log"), "x".repeat(1_000_001)).unwrap();
        log_line(&dir, "yeni");
        assert!(dir.join("app.log.1").exists(), "eski günlük devredilmeli");
        assert!(std::fs::read_to_string(dir.join("app.log"))
            .unwrap()
            .contains("yeni"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
