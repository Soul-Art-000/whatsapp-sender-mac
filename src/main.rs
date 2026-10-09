#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{command, AppHandle, Manager, State};

use wa_rs::bot::Bot;
use wa_rs::types::events::Event;
use wa_rs_sqlite_storage::SqliteStore;
use wa_rs_tokio_transport::TokioWebSocketTransportFactory;
use wa_rs_ureq_http::UreqHttpClient;

struct AppState {
    db: Mutex<Connection>,
    client: Mutex<Option<Arc<wa_rs::Client>>>,
    /// Gönderim sırasında "Durdur" için.
    stop: Arc<AtomicBool>,
    /// Aynı anda iki kez bağlanmayı engeller.
    connecting: Arc<AtomicBool>,
}

// ==== SHARED BEGIN ====
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Contact {
    id: i64,
    name: String,
    phone: String,
}

#[derive(Serialize, Clone)]
struct SendSummary {
    sent: usize,
    failed: usize,
}

#[derive(Serialize, Clone)]
struct ImportResult {
    added: usize,
    duplicates: usize,
}

// ---------------------------------------------------------------- telefon

/// Ham bir telefon numarasını WhatsApp'ın beklediği `905551112233` biçimine
/// çevirir. Numarayı tanıyamazsa `None` döner.
///
/// Desteklenen girdiler: `+90 555 123 45 67`, `0555 123 45 67`,
/// `5551234567`, `00905551234567`, `(053) 785 377 70`.
fn normalize_phone(raw: &str) -> Option<String> {
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
fn parse_vcf(content: &str) -> Vec<(String, String)> {
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
        let Some(colon) = line.find(':') else { continue };
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
fn parse_csv(content: &str) -> Vec<(String, String)> {
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

fn unescape_vcard(s: &str) -> String {
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
fn insert_contacts(
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

/// 4-9 saniye arası rastgele anti-ban beklemesi.
fn random_delay_secs() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    4 + (nanos % 6)
}

fn mime_for(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

fn build_text_message(text: &str, contact_name: &str) -> wa_rs::wa_rs_proto::whatsapp::Message {
    let mut msg = wa_rs::wa_rs_proto::whatsapp::Message::default();
    msg.extended_text_message = Some(Box::new(
        wa_rs::wa_rs_proto::whatsapp::message::ExtendedTextMessage {
            text: Some(text.replace("/isim", contact_name)),
            ..Default::default()
        },
    ));
    msg
}

fn build_image_message(
    text: &str,
    contact_name: &str,
    mime: &str,
    up: wa_rs::upload::UploadResponse,
) -> wa_rs::wa_rs_proto::whatsapp::Message {
    let mut msg = wa_rs::wa_rs_proto::whatsapp::Message::default();
    msg.image_message = Some(Box::new(
        wa_rs::wa_rs_proto::whatsapp::message::ImageMessage {
            url: Some(up.url),
            mimetype: Some(mime.to_string()),
            caption: Some(text.replace("/isim", contact_name)),
            file_sha256: Some(up.file_sha256),
            file_length: Some(up.file_length),
            media_key: Some(up.media_key),
            file_enc_sha256: Some(up.file_enc_sha256),
            direct_path: Some(up.direct_path),
            ..Default::default()
        },
    ));
    msg
}

/// Toplu gönderim döngüsü. Tauri'den bağımsız olsun diye `emit` bir closure.
/// Tek bir numara hata verse bile kalanlara devam eder; eski sürüm ilk hatada
/// tüm listeyi iptal ediyordu. Resim de kişi başına değil bir kez yüklenir.
async fn send_batch<F: Fn(String) + Send>(
    client: Arc<wa_rs::Client>,
    contacts: &[Contact],
    message: &str,
    image_path: Option<&str>,
    stop: &AtomicBool,
    emit: F,
) -> Result<SendSummary, String> {
    let upload = match image_path {
        Some(p) => {
            emit("Resim yükleniyor...".to_string());
            let data = std::fs::read(p).map_err(|e| format!("Resim okunamadı: {}", e))?;
            let mime = mime_for(p);
            let up = client
                .upload(data, wa_rs::download::MediaType::Image)
                .await
                .map_err(|e| format!("Resim yüklenemedi: {}", e))?;
            Some((up, mime))
        }
        None => None,
    };

    let mut summary = SendSummary { sent: 0, failed: 0 };

    for (i, contact) in contacts.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            emit(format!(
                "Durduruldu. {}/{} gönderildi.",
                summary.sent,
                contacts.len()
            ));
            return Ok(summary);
        }

        emit(format!(
            "Gönderiliyor... ({}/{}) - {}",
            i + 1,
            contacts.len(),
            contact.name
        ));

        let msg = match &upload {
            Some((up, mime)) => build_image_message(message, &contact.name, mime, up.clone()),
            None => build_text_message(message, &contact.name),
        };

        let jid_str = format!("{}@s.whatsapp.net", contact.phone);
        let jid = match std::str::FromStr::from_str(&jid_str) {
            Ok(j) => j,
            Err(_) => {
                summary.failed += 1;
                emit(format!("Atlandı (geçersiz numara): {}", contact.name));
                continue;
            }
        };

        match client.send_message(jid, msg).await {
            Ok(_) => summary.sent += 1,
            Err(e) => {
                summary.failed += 1;
                emit(format!("Hata ({}): {}", contact.name, e));
            }
        }

        // Son mesajdan sonra beklemeye gerek yok
        if i + 1 < contacts.len() {
            let secs = random_delay_secs();
            emit(format!("{} sn bekleniyor...", secs));
            tokio::time::sleep(Duration::from_secs(secs)).await;
        }
    }

    Ok(summary)
}

// ------------------------------------------------------------------ olaylar

/// wa-rs olaylarını arayüze taşınacak (isim, mesaj) çiftine çevirir.
/// Eski sürüm "bağlı" bilgisini neredeyse her olayda gönderdiği için arayüz
/// bağlantı kopmuş olsa bile "Bağlı (Hazır)" gösteriyordu.
fn map_event(event: &Event) -> Option<(&'static str, String)> {
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
        Event::TemporaryBan(b) => Some(("whatsapp_error", format!("GEÇİCİ BAN: {}", ban_reason_tr(&b.code)))),
        Event::ConnectFailure(f) => Some(("whatsapp_error", format!("Bağlantı hatası: {}", f.message))),
        Event::StreamReplaced(_) => Some((
            "whatsapp_error",
            "Oturum başka bir cihazdan açıldı.".to_string(),
        )),
        Event::StreamError(e) => Some(("whatsapp_error", format!("Akış hatası: {}", e.code))),
        _ => None,
    }
}

fn ban_reason_tr(reason: &wa_rs::types::events::TempBanReason) -> &'static str {
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

fn init_db(app_dir: &std::path::Path) -> Result<Connection, String> {
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
// ==== SHARED END ====

#[command]
fn get_contacts(state: State<AppState>) -> Result<Vec<Contact>, String> {
    let db = state.db.lock().unwrap();
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
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[command]
fn add_contact(name: String, phone: String, state: State<AppState>) -> Result<(), String> {
    let normalized = normalize_phone(&phone)
        .ok_or_else(|| format!("Geçersiz telefon numarası: {}", phone))?;
    let name = if name.trim().is_empty() {
        "İsimsiz".to_string()
    } else {
        name.trim().to_string()
    };
    let db = state.db.lock().unwrap();
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

#[command]
fn remove_contact(phone: String, state: State<AppState>) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute("DELETE FROM contacts WHERE phone = ?1", [&phone])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[command]
fn import_contacts_from_file(path: String, state: State<AppState>) -> Result<ImportResult, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("Dosya okunamadı: {}", e))?;
    let content = String::from_utf8_lossy(&bytes);

    let pairs = if path.to_lowercase().ends_with(".csv") {
        parse_csv(&content)
    } else {
        parse_vcf(&content)
    };
    if pairs.is_empty() {
        return Err("Dosyada geçerli bir isim/telefon bulunamadı.".to_string());
    }

    let mut db = state.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let result = insert_contacts(&tx, &pairs).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(result)
}

#[command]
async fn connect_whatsapp(app_handle: AppHandle) -> Result<(), String> {
    let connecting = {
        let state = app_handle.state::<AppState>();
        if state.client.lock().unwrap().is_some() {
            return Ok(()); // zaten bağlı
        }
        if state.connecting.swap(true, Ordering::SeqCst) {
            return Ok(()); // bağlanma zaten sürüyor
        }
        state.connecting.clone()
    };

    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_bot(handle.clone()).await {
            let _ = handle.emit_all("whatsapp_error", e);
        }
        connecting.store(false, Ordering::SeqCst);
        let state = handle.state::<AppState>();
        *state.client.lock().unwrap() = None;
    });

    Ok(())
}

async fn run_bot(app_handle: AppHandle) -> Result<(), String> {
    let app_dir = app_handle
        .path_resolver()
        .app_data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;

    let store_path = app_dir.join("wa_bot.db");
    let backend = Arc::new(
        SqliteStore::new(&store_path.to_string_lossy())
            .await
            .map_err(|e| format!("WhatsApp oturumu açılamadı: {}", e))?,
    );

    let handler_app = app_handle.clone();
    let mut bot = Bot::builder()
        .with_backend(backend)
        .with_transport_factory(TokioWebSocketTransportFactory::new())
        .with_http_client(UreqHttpClient::new())
        .on_event(move |event, _client| {
            let app = handler_app.clone();
            async move {
                if let Some((name, payload)) = map_event(&event) {
                    let _ = app.emit_all(name, payload);
                }
            }
        })
        .build()
        .await
        .map_err(|e| format!("WhatsApp istemcisi başlatılamadı: {}", e))?;

    let client = bot.client();
    *app_handle.state::<AppState>().client.lock().unwrap() = Some(client);
    let _ = app_handle.emit_all("whatsapp_connecting", ());

    bot.run()
        .await
        .map_err(|e| format!("WhatsApp bağlantısı kurulamadı: {}", e))?
        .await;
    Ok(())
}

#[command]
async fn send_whatsapp_messages(
    contacts: Vec<Contact>,
    message: String,
    image_path: Option<String>,
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<SendSummary, String> {
    let client = state
        .client
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "WhatsApp bağlı değil! Lütfen önce QR kodu okutun.".to_string())?;

    state.stop.store(false, Ordering::SeqCst);
    let stop = state.stop.clone();

    let emit_handle = app_handle.clone();
    let emit = move |s: String| {
        let _ = emit_handle.emit_all("send_status", s);
    };

    let summary = send_batch(
        client,
        &contacts,
        &message,
        image_path.as_deref(),
        &stop,
        emit,
    )
    .await?;

    let _ = app_handle.emit_all("send_done", summary.clone());
    Ok(summary)
}

#[command]
fn stop_sending(state: State<'_, AppState>) {
    state.stop.store(true, Ordering::SeqCst);
}

// ------------------------------------------------------------------ gruplar

#[derive(Serialize)]
struct Group {
    name: String,
    contact_ids: Vec<i64>,
}

#[command]
fn save_group(name: String, contact_ids: Vec<i64>, state: State<'_, AppState>) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Grup adı boş olamaz.".to_string());
    }
    let ids_str = contact_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<String>>()
        .join(",");
    let db = state.db.lock().unwrap();
    db.execute(
        "INSERT OR REPLACE INTO groups (name, contact_ids) VALUES (?1, ?2)",
        params![name, ids_str],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[command]
fn get_groups(state: State<'_, AppState>) -> Result<Vec<Group>, String> {
    let db = state.db.lock().unwrap();
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
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[command]
fn delete_group(name: String, state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute("DELETE FROM groups WHERE name = ?1", [name])
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let app_dir = app.path_resolver().app_data_dir().unwrap();
            std::fs::create_dir_all(&app_dir).unwrap();
            let db = init_db(&app_dir)?;
            app.manage(AppState {
                db: Mutex::new(db),
                client: Mutex::new(None),
                stop: Arc::new(AtomicBool::new(false)),
                connecting: Arc::new(AtomicBool::new(false)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_contacts,
            add_contact,
            remove_contact,
            import_contacts_from_file,
            connect_whatsapp,
            send_whatsapp_messages,
            stop_sending,
            save_group,
            get_groups,
            delete_group
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
