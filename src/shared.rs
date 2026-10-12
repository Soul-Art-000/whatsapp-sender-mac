//! Çekirdek mantık — Tauri'den bağımsız.
//!
//! Aynı kod iki yerden kullanılır: masaüstü uygulaması (`src/main.rs`, Tauri
//! komut katmanı) ve sunucu ikilisi (`src/bin/whatsapp-server.rs`, HTTP/WS
//! katmanı). Böylece gönderim/anti-ban/import mantığı tek yerde kalır.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
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
}

impl AppState {
    pub fn new(db: Connection) -> Self {
        AppState {
            db: Mutex::new(db),
            client: Mutex::new(None),
            stop: Arc::new(AtomicBool::new(false)),
            connecting: Arc::new(AtomicBool::new(false)),
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
pub fn random_delay_secs() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    4 + (nanos % 6)
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

/// Bir kişiye gönderilecek mesajları kurar. Metin varsa tek metin mesajı;
/// resim varsa ilki mesaj metniyle birlikte, kalanlar ayrı mesaj olarak.
pub fn build_messages(
    message: &str,
    contact_name: &str,
    uploads: &[(wa_rs::upload::UploadResponse, &'static str)],
) -> Vec<wa_rs::wa_rs_proto::whatsapp::Message> {
    if uploads.is_empty() {
        return vec![build_text_message(message, contact_name)];
    }
    uploads
        .iter()
        .enumerate()
        .map(|(idx, (up, mime))| {
            let caption = if idx == 0 { message } else { "" };
            build_image_message(caption, contact_name, mime, up.clone())
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
/// - Resmi bir kez yükler, "Durdur" ile bekleme sırasında da kesilebilir.
pub async fn send_batch<F: Fn(String) + Send>(
    client: Arc<wa_rs::Client>,
    contacts: &[Contact],
    message: &str,
    image_paths: &[String],
    validate: bool,
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

    // 1) Resimleri bir kez yükle (kişi başına tekrar yüklenmez).
    let mut uploads: Vec<(wa_rs::upload::UploadResponse, &'static str)> = Vec::new();
    for (idx, p) in image_paths.iter().enumerate() {
        emit(format!(
            "Resim yükleniyor... ({}/{})",
            idx + 1,
            image_paths.len()
        ));
        let data = std::fs::read(p).map_err(|e| format!("Resim okunamadı ({}): {}", p, e))?;
        let mime = mime_for(p);
        let up = client
            .upload(data, wa_rs::download::MediaType::Image)
            .await
            .map_err(|e| format!("Resim yüklenemedi ({}): {}", p, e))?;
        uploads.push((up, mime));
    }

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

        // İlk resim mesaj metniyle birlikte, kalan resimler ayrı mesaj olarak gider.
        let msgs = build_messages(message, &contact.name, &uploads);

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

pub fn mime_for(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else {
        "image/jpeg"
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
        Event::Connected(_) | Event::PairSuccess(_) => Some(("whatsapp_ready", String::new())),
        Event::Disconnected(_) => Some((
            "whatsapp_disconnected",
            "Bağlantı koptu, yeniden bağlanılıyor...".to_string(),
        )),
        Event::LoggedOut(_) => Some((
            "whatsapp_error",
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
pub async fn run_bot<E, F>(app_dir: &Path, emit: E, on_client: F) -> Result<(), String>
where
    E: Fn(&str, serde_json::Value) + Clone + Send + Sync + 'static,
    F: FnOnce(Arc<wa_rs::Client>) + Send + 'static,
{
    std::fs::create_dir_all(app_dir).map_err(|e| e.to_string())?;
    let store_path = app_dir.join("wa_bot.db");
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
    emit("whatsapp_connecting", serde_json::Value::Null);

    let _ = bot
        .run()
        .await
        .map_err(|e| format!("WhatsApp bağlantısı kurulamadı: {}", e))?
        .await;
    Ok(())
}

/// Sadece bağlantıyı keser; oturum korunur, bir sonraki açılışta QR gerekmez.
pub async fn disconnect_session(state: &AppState) -> Result<(), String> {
    let client = state
        .client
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Zaten bağlı değil.".to_string())?;
    client.disconnect().await;
    Ok(())
}

/// İstemci tamamen durana kadar bekler (oturum dosyası açıkken silinemez).
pub async fn wait_client_stopped(state: &AppState) {
    for _ in 0..60 {
        if !state.connecting.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Yerel WhatsApp oturumunu siler (çıkış). wa-rs 0.2.0 sunucu tarafında cihazı
/// kaldıran bir API sunmuyor: bağlantı kesilir ve yerel oturum dosyaları silinir,
/// böylece bir sonraki bağlantıda QR yeniden okutulur. Telefondaki "Bağlı
/// cihazlar" kaydını kullanıcı kaldırır.
pub fn delete_wa_session(app_dir: &Path) -> Result<(), String> {
    for name in ["wa_bot.db", "wa_bot.db-wal", "wa_bot.db-shm"] {
        let path = app_dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Oturum dosyası silinemedi ({}): {}", name, e))?;
        }
    }
    Ok(())
}
