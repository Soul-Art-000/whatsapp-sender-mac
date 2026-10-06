#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{State, command, AppHandle, Manager};
use rusqlite::Connection;
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::sleep;

use wa_rs::bot::Bot;
use wa_rs::types::events::Event;
use wa_rs_sqlite_storage::SqliteStore;
use wa_rs_tokio_transport::TokioWebSocketTransportFactory;
use wa_rs_ureq_http::UreqHttpClient;
use std::sync::Arc;

struct AppState {
    db: Mutex<Connection>,
    client: Mutex<Option<Arc<wa_rs::Client>>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Contact {
    id: i64,
    name: String,
    phone: String,
}

#[command]
fn get_contacts(state: State<AppState>) -> Result<Vec<Contact>, String> {
    let db = state.db.lock().unwrap();
    let mut stmt = db.prepare("SELECT id, name, phone FROM contacts ORDER BY id DESC").map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |row| {
        Ok(Contact {
            id: row.get(0)?,
            name: row.get(1)?,
            phone: row.get(2)?,
        })
    }).map_err(|e| e.to_string())?;

    let mut contacts = Vec::new();
    for r in rows {
        if let Ok(c) = r { contacts.push(c); }
    }
    Ok(contacts)
}

#[command]
fn add_contact(name: String, phone: String, state: State<AppState>) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute("INSERT OR IGNORE INTO contacts (name, phone) VALUES (?1, ?2)", [&name, &phone]).map_err(|e| e.to_string())?;
    Ok(())
}

#[command]
fn remove_contact(phone: String, state: State<AppState>) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute("DELETE FROM contacts WHERE phone = ?1", [&phone]).map_err(|e| e.to_string())?;
    Ok(())
}

#[command]
fn import_contacts_from_file(path: String, state: State<AppState>) -> Result<usize, String> {
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut db = state.db.lock().unwrap();
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let mut count = 0;

    let path_lower = path.to_lowercase();
    if path_lower.ends_with(".csv") {
        let mut rdr = csv::ReaderBuilder::new().flexible(true).from_reader(content.as_bytes());
        for result in rdr.records() {
            if let Ok(record) = result {
                let mut name = String::new();
                let mut phone = String::new();
                for field in record.iter() {
                    let cleaned: String = field.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
                    if cleaned.len() >= 10 {
                        phone = cleaned;
                    } else if name.is_empty() {
                        name = field.to_string();
                    }
                }
                if !phone.is_empty() {
                    if name.is_empty() { name = "İsimsiz".to_string(); }
                    if tx.execute("INSERT OR IGNORE INTO contacts (name, phone) VALUES (?1, ?2)", [&name, &phone]).is_ok() {
                        count += 1;
                    }
                }
            }
        }
    } else {
        // Simple VCF parsing: look for FN: and TEL:
        let mut current_name = String::new();
        for line in content.lines() {
            let line_upper = line.to_uppercase();
            if line_upper.starts_with("FN") {
                if let Some(idx) = line.find(':') {
                    current_name = line[idx + 1..].trim().to_string();
                }
            } else if line_upper.starts_with("TEL") {
                if let Some(idx) = line.find(':') {
                    let val = &line[idx + 1..];
                    let cleaned: String = val.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
                    if cleaned.len() >= 10 {
                        let n = if current_name.is_empty() { "İsimsiz".to_string() } else { current_name.clone() };
                        if tx.execute("INSERT OR IGNORE INTO contacts (name, phone) VALUES (?1, ?2)", [&n, &cleaned]).is_ok() {
                            count += 1;
                        }
                    }
                }
            } else if line_upper.starts_with("END:VCARD") {
                current_name.clear();
            }
        }
    }

    tx.commit().map_err(|e| e.to_string())?;
    Ok(count)
}

#[command]
async fn connect_whatsapp(app_handle: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn(async move {
        let app_dir = app_handle.path_resolver().app_data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let db_path = app_dir.join("wa_bot.db");
        let backend = Arc::new(SqliteStore::new(&db_path.to_string_lossy()).await.unwrap());
        
        let app_handle_clone = app_handle.clone();
        let mut bot = Bot::builder()
            .with_backend(backend)
            .with_transport_factory(TokioWebSocketTransportFactory::new())
            .with_http_client(UreqHttpClient::new())
            .on_event(move |event, _client| {
                let app_handle = app_handle_clone.clone();
                async move {
                    match event {
                        Event::PairingQrCode { code, .. } => {
                            let _ = app_handle.emit_all("qr_code", code);
                        },
                        Event::Message(msg, info) => {
                            let _ = app_handle.emit_all("whatsapp_ready", ());
                        },
                        _ => {
                            let _ = app_handle.emit_all("whatsapp_ready", ());
                        }
                    }
                }
            })
            .build()
            .await
            .unwrap();
            
        let client = bot.client();
        let app_state = app_handle.state::<AppState>();
        *app_state.client.lock().unwrap() = Some(client);
            
        let _ = bot.run().await.unwrap().await;
    });
    
    Ok(())
}

#[command]
async fn send_whatsapp_messages(contacts: Vec<Contact>, message: String, image_path: Option<String>, state: State<'_, AppState>, app_handle: AppHandle) -> Result<(), String> {
    let client_opt = state.client.lock().unwrap().clone();
    let client = client_opt.ok_or_else(|| "WhatsApp bağlı değil! Lütfen önce QR kod okutun.".to_string())?;

    for (i, contact) in contacts.iter().enumerate() {
        let _ = app_handle.emit_all("send_status", format!("Gönderiliyor... ({}/{}) - {}", i+1, contacts.len(), contact.name));
        
        let delay = 4 + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() % 6) as u64;
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        
        let text = message.replace("/isim", &contact.name);
        
        let mut clean_phone = contact.phone.replace("+", "").replace(" ", "").replace("-", "").replace("(", "").replace(")", "");
        if clean_phone.starts_with("0") {
            clean_phone = format!("90{}", &clean_phone[1..]);
        } else if clean_phone.starts_with("5") && clean_phone.len() == 10 {
            clean_phone = format!("90{}", clean_phone);
        }
        
        let jid_str = format!("{}@s.whatsapp.net", clean_phone);
        let jid = std::str::FromStr::from_str(&jid_str).map_err(|_| format!("Geçersiz telefon formatı: {}", jid_str))?;
        
        let mut msg = wa_rs::wa_rs_proto::whatsapp::Message::default();

        if let Some(ref img_path) = image_path {
            let data = std::fs::read(img_path).map_err(|e| format!("Resim okunamadı: {}", e))?;
            println!("Resim yükleniyor: {:?}", img_path);
            
            let upload_response = client.upload(data, wa_rs::download::MediaType::Image).await.map_err(|e| format!("Resim yüklenemedi: {}", e))?;
            
            msg.image_message = Some(Box::new(wa_rs::wa_rs_proto::whatsapp::message::ImageMessage {
                url: Some(upload_response.url),
                mimetype: Some("image/jpeg".to_string()),
                caption: Some(text),
                file_sha256: Some(upload_response.file_sha256),
                file_length: Some(upload_response.file_length),
                media_key: Some(upload_response.media_key),
                file_enc_sha256: Some(upload_response.file_enc_sha256),
                direct_path: Some(upload_response.direct_path),
                ..Default::default()
            }));
        } else {
            msg.extended_text_message = Some(Box::new(wa_rs::wa_rs_proto::whatsapp::message::ExtendedTextMessage {
                text: Some(text),
                ..Default::default()
            }));
        }

        println!("Sending to JID: {}", jid_str);
        match client.send_message(jid, msg).await {
            Ok(msg_id) => println!("Success! Msg ID: {}", msg_id),
            Err(e) => {
                println!("Error sending message: {}", e);
                return Err(e.to_string());
            }
        }
    }
    
    let _ = app_handle.emit_all("send_status", "Tamamlandı!");
    Ok(())
}

#[derive(serde::Serialize)]
struct Group {
    name: String,
    contact_ids: Vec<i64>,
}

#[command]
fn save_group(name: String, contact_ids: Vec<i64>, state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    let ids_str = contact_ids.iter().map(|id| id.to_string()).collect::<Vec<String>>().join(",");
    db.execute("INSERT OR REPLACE INTO groups (name, contact_ids) VALUES (?1, ?2)", [name, ids_str]).map_err(|e| e.to_string())?;
    Ok(())
}

#[command]
fn get_groups(state: State<'_, AppState>) -> Result<Vec<Group>, String> {
    let db = state.db.lock().unwrap();
    let mut stmt = db.prepare("SELECT name, contact_ids FROM groups").map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |row| {
        let name: String = row.get(0)?;
        let ids_str: String = row.get(1)?;
        let contact_ids = ids_str.split(',').filter_map(|s| s.parse::<i64>().ok()).collect();
        Ok(Group { name, contact_ids })
    }).map_err(|e| e.to_string())?;
    
    let mut groups = Vec::new();
    for row in rows {
        groups.push(row.unwrap());
    }
    Ok(groups)
}

#[command]
fn delete_group(name: String, state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute("DELETE FROM groups WHERE name = ?1", [name]).map_err(|e| e.to_string())?;
    Ok(())
}

fn init_db(app_dir: &std::path::Path) -> Connection {
    let db_path = app_dir.join("contacts.sqlite");
    let db = Connection::open(db_path).expect("Failed to open SQLite database");
    db.execute("CREATE TABLE IF NOT EXISTS contacts (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, phone TEXT UNIQUE)", []).unwrap();
    db.execute("CREATE TABLE IF NOT EXISTS groups (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT UNIQUE, contact_ids TEXT)", []).unwrap();
    db
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let app_dir = app.path_resolver().app_data_dir().unwrap();
            std::fs::create_dir_all(&app_dir).unwrap();
            let db = init_db(&app_dir);
            app.manage(AppState { 
                db: Mutex::new(db),
                client: Mutex::new(None),
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
            save_group,
            get_groups,
            delete_group
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
