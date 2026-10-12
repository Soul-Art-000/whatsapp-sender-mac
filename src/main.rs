#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Masaüstü (Tauri) katmanı — ince sarmalayıcı.
//!
//! Gerçek mantık `whatsapp_sender::shared` içinde; burada yalnızca Tauri
//! komutları, olay yayını ve pencere kurulumu var. Aynı çekirdek sunucu
//! ikilisinde (`src/bin/whatsapp-server.rs`) HTTP/WS katmanıyla kullanılır.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use tauri::{command, AppHandle, Manager, State};

use whatsapp_sender::shared::{self, AppState, Contact, Group, ImportResult, SendSummary};

fn data_dir(app: &AppHandle) -> PathBuf {
    app.path_resolver()
        .app_data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
}

// ------------------------------------------------------------------ kişiler

#[command]
fn get_contacts(state: State<AppState>) -> Result<Vec<Contact>, String> {
    shared::get_contacts(&state.db.lock().unwrap())
}

#[command]
fn add_contact(name: String, phone: String, state: State<AppState>) -> Result<(), String> {
    shared::add_contact(&state.db.lock().unwrap(), &name, &phone)
}

#[command]
fn remove_contact(phone: String, state: State<AppState>) -> Result<(), String> {
    shared::remove_contact(&state.db.lock().unwrap(), &phone)
}

#[command]
fn clear_contacts(state: State<'_, AppState>) -> Result<usize, String> {
    shared::clear_contacts(&state.db.lock().unwrap())
}

#[command]
fn import_contacts_from_file(path: String, state: State<AppState>) -> Result<ImportResult, String> {
    let mut db = state.db.lock().unwrap();
    shared::import_contacts_from_file(&mut db, std::path::Path::new(&path))
}

// ---------------------------------------------------------------- whatsapp

#[command]
async fn connect_whatsapp(app_handle: AppHandle) -> Result<(), String> {
    spawn_connect(app_handle);
    Ok(())
}

/// Arayüzün açılışta doğru durumu göstermesi için.
#[command]
fn get_wa_status(state: State<'_, AppState>) -> String {
    shared::wa_status(&state.client.lock().unwrap())
}

#[command]
async fn disconnect_whatsapp(state: State<'_, AppState>) -> Result<(), String> {
    shared::disconnect_session(&state).await
}

/// WhatsApp oturumunu bu bilgisayardan siler (çıkış).
///
/// wa-rs 0.2.0 sunucu tarafında cihazı kaldıran bir API sunmuyor, bu yüzden
/// yapılan şey: bağlantıyı kesip yerel oturum dosyalarını silmek. Telefondaki
/// "Bağlı cihazlar" kaydını kullanıcı telefonundan kaldırır (arayüz söylüyor).
#[command]
async fn logout_whatsapp(app_handle: AppHandle) -> Result<(), String> {
    let client = {
        let state = app_handle.state::<AppState>();
        // Not: değeri ara değişkende tutmak gerekiyor; doğrudan blok sonunda
        // dönerse geçici MutexGuard `state`ten sonra düşer ve E0597 verir.
        let c = state.client.lock().unwrap().clone();
        c
    };
    if let Some(c) = client {
        c.disconnect().await;
    }
    {
        let state = app_handle.state::<AppState>();
        shared::wait_client_stopped(&state).await;
    }
    shared::delete_wa_session(&data_dir(&app_handle))?;
    let _ = app_handle.emit_all("whatsapp_logged_out", ());
    Ok(())
}

/// WhatsApp istemcisini başlatır (arka planda).
fn spawn_connect(app_handle: AppHandle) {
    let connecting = {
        let state = app_handle.state::<AppState>();
        if state.client.lock().unwrap().is_some() {
            return; // zaten bağlı
        }
        if state.connecting.swap(true, Ordering::SeqCst) {
            return; // bağlanma zaten sürüyor
        }
        state.connecting.clone()
    };

    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let emit_handle = handle.clone();
        let emit = move |name: &str, payload: serde_json::Value| {
            let _ = emit_handle.emit_all(name, payload);
        };
        let client_handle = handle.clone();
        let on_client = move |client: std::sync::Arc<wa_rs::Client>| {
            *client_handle.state::<AppState>().client.lock().unwrap() = Some(client);
        };

        if let Err(e) = shared::run_bot(&data_dir(&handle), emit, on_client).await {
            let _ = handle.emit_all("whatsapp_error", e);
        }
        connecting.store(false, Ordering::SeqCst);
        *handle.state::<AppState>().client.lock().unwrap() = None;
        let _ = handle.emit_all("whatsapp_disconnected", "Bağlantı kapandı.".to_string());
    });
}

// ---------------------------------------------------------------- gönderim

#[command]
async fn send_whatsapp_messages(
    contacts: Vec<Contact>,
    message: String,
    image_paths: Vec<String>,
    validate: bool,
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

    let summary = shared::send_batch(
        client,
        &contacts,
        &message,
        &image_paths,
        validate,
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

#[command]
fn save_group(
    name: String,
    contact_ids: Vec<i64>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    shared::save_group(&state.db.lock().unwrap(), &name, &contact_ids)
}

#[command]
fn get_groups(state: State<'_, AppState>) -> Result<Vec<Group>, String> {
    shared::get_groups(&state.db.lock().unwrap())
}

#[command]
fn delete_group(name: String, state: State<'_, AppState>) -> Result<(), String> {
    shared::delete_group(&state.db.lock().unwrap(), &name)
}

// -------------------------------------------------------------------- main

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let app_dir = data_dir(&app.handle());
            std::fs::create_dir_all(&app_dir).unwrap();
            let db = shared::init_db(&app_dir)?;
            app.manage(AppState::new(db));
            // Oturum daha önce eşleşmişse açılışta kendiliğinden bağlan
            // (QR kod her seferinde yeniden okutulmasın).
            if app_dir.join("wa_bot.db").exists() {
                spawn_connect(app.handle());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_contacts,
            add_contact,
            remove_contact,
            clear_contacts,
            import_contacts_from_file,
            connect_whatsapp,
            get_wa_status,
            disconnect_whatsapp,
            logout_whatsapp,
            send_whatsapp_messages,
            stop_sending,
            save_group,
            get_groups,
            delete_group
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
