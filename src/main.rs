#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Masaüstü (Tauri) katmanı — ince sarmalayıcı.
//!
//! Gerçek mantık `whatsapp_sender::shared` içinde; burada yalnızca Tauri
//! komutları, olay yayını ve pencere kurulumu var. Aynı çekirdek sunucu
//! ikilisinde (`src/bin/whatsapp-server.rs`) HTTP/WS katmanıyla kullanılır.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{
    command, AppHandle, CustomMenuItem, Manager, State, SystemTray, SystemTrayEvent,
    SystemTrayMenu, SystemTrayMenuItem, WindowEvent,
};

use whatsapp_sender::shared::{
    self, AppState, Contact, Group, ImportResult, ScheduledJob, SendSummary,
};

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
    // Zaten bağlıysa sessizce hiçbir şey yapmıyordu: arayüz "QR bekleniyor"da
    // kalıyordu. Şimdi ya eldeki QR gösterilir ya da durum açıkça söylenir.
    let (client_ok, qr) = {
        let state = app_handle.state::<AppState>();
        let c = state.client.lock().unwrap().clone();
        let logged_in = c
            .as_ref()
            .map(|c| c.is_connected() && c.is_logged_in())
            .unwrap_or(false);
        let qr = state.last_qr.lock().unwrap().clone();
        (logged_in, qr)
    };

    if client_ok {
        return Err(
            "WhatsApp zaten bağlı. Başka bir cihaz/numara bağlamak için önce 'Çıkış' ile \
             oturumu silin, sonra 'WhatsApp Bağla'ya basın."
                .to_string(),
        );
    }

    match qr {
        Some(code) => {
            shared::log_line(&data_dir(&app_handle), "bekleyen QR tekrar gösterildi");
            let _ = app_handle.emit_all("qr_code", serde_json::Value::String(code));
        }
        None => spawn_connect(app_handle),
    }
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
    let dir = data_dir(&app_handle);
    let state = app_handle.state::<AppState>();
    shared::log_line(&dir, "çıkış: oturum siliniyor");
    let sonuc = shared::logout_session(&state, &dir).await;
    match &sonuc {
        Ok(()) => shared::log_line(&dir, "çıkış: oturum silindi"),
        Err(e) => shared::log_line(&dir, &format!("çıkış HATASI: {}", e)),
    }
    sonuc?;
    *state.last_qr.lock().unwrap() = None;
    let _ = app_handle.emit_all("whatsapp_logged_out", ());
    Ok(())
}

/// WhatsApp istemcisini başlatır (arka planda).
fn spawn_connect(app_handle: AppHandle) {
    // Her bağlanma yeni bir kuşak açar; önceki turun kapanışı artık etkisiz.
    let running = {
        let state = app_handle.state::<AppState>();
        let token = Arc::new(AtomicBool::new(true));
        *state.running.lock().unwrap() = Some(token.clone());
        token
    };
    let stop_slot = app_handle.state::<AppState>().stop_handle.clone();
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
        // `last_qr`: QR arayüz açılmadan önce üretilirse kaybolmasın diye
        // saklanır (sunucu modunda da böyle). Günlük: teşhis için app.log.
        let emit = move |name: &str, payload: serde_json::Value| {
            let dir = data_dir(&emit_handle);
            match name {
                "qr_code" => {
                    if let Some(code) = payload.as_str() {
                        *emit_handle.state::<AppState>().last_qr.lock().unwrap() =
                            Some(code.to_string());
                    }
                }
                "whatsapp_ready" | "whatsapp_logged_out" => {
                    *emit_handle.state::<AppState>().last_qr.lock().unwrap() = None;
                }
                _ => {}
            }
            shared::log_line(&dir, &format!("olay {} {}", name, payload));
            let _ = emit_handle.emit_all(name, payload);
        };
        let client_handle = handle.clone();
        let on_client = move |client: std::sync::Arc<wa_rs::Client>| {
            *client_handle.state::<AppState>().client.lock().unwrap() = Some(client);
        };

        let dir = data_dir(&handle);
        shared::log_line(&dir, "bağlantı başlatıldı");
        let result = shared::run_bot(&dir, emit, on_client, stop_slot).await;
        // Bu tur `running` sırasında "Kes"/"Çıkış" ile öldürülmüşse kapanış
        // temizliği yapılmaz: aksi hâlde o arada başlamış YENİ bağlantı
        // `client = None` yapılıp arayüz "offline"a düşüyor, kullanıcı çıkış
        // yaptığını sanırken uygulama yine WhatsApp'a bağlı kalıyordu.
        if !running.load(Ordering::SeqCst) {
            connecting.store(false, Ordering::SeqCst);
            return;
        }
        if let Err(e) = result {
            shared::log_line(&dir, &format!("bağlantı hatası: {}", e));
            let _ = handle.emit_all("whatsapp_error", e);
        }
        connecting.store(false, Ordering::SeqCst);
        *handle.state::<AppState>().client.lock().unwrap() = None;
        shared::log_line(&dir, "bağlantı kapandı");
        let _ = handle.emit_all("whatsapp_disconnected", "Bağlantı kapandı.".to_string());
    });
}

// ---------------------------------------------------------------- gönderim

#[command]
async fn send_whatsapp_messages(
    contacts: Vec<Contact>,
    message: String,
    attachment_paths: Vec<String>,
    validate: bool,
    #[allow(clippy::too_many_arguments)] view_once: Option<bool>,
    state: State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<SendSummary, String> {
    let client = state
        .client
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "WhatsApp bağlı değil! Lütfen önce QR kodu okutun.".to_string())?;

    // Aynı anda tek gönderim: zamanlanmış iş sürerken yeni bir toplu gönderim
    // başlatılırsa ikisi de aynı WhatsApp oturumunu bozar.
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("Şu anda başka bir gönderim sürüyor. Bitmesini bekleyin.".to_string());
    }

    state.stop.store(false, Ordering::SeqCst);
    let stop = state.stop.clone();

    let emit_handle = app_handle.clone();
    let emit = move |s: String| {
        let _ = emit_handle.emit_all("send_status", s);
    };

    let result = shared::send_batch(
        client,
        &contacts,
        &message,
        &attachment_paths,
        validate,
        view_once.unwrap_or(false),
        &stop,
        emit,
    )
    .await;
    state.busy.store(false, Ordering::SeqCst);
    let summary = result?;

    let _ = app_handle.emit_all("send_done", summary.clone());
    Ok(summary)
}

#[command]
fn stop_sending(state: State<'_, AppState>) {
    state.stop.store(true, Ordering::SeqCst);
}

// ------------------------------------------------------------- zamanlanmış

/// Mesajı kuyruğa yazar; gönderim ayrı bir arka plan turunda yapılır.
#[command]
fn schedule_messages(
    contacts: Vec<Contact>,
    message: String,
    attachment_paths: Vec<String>,
    send_at: u64,
    validate: bool,
    view_once: Option<bool>,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    shared::add_scheduled(
        &state.db.lock().unwrap(),
        send_at,
        &message,
        &attachment_paths,
        &contacts,
        validate,
        view_once.unwrap_or(false),
    )
}

#[command]
fn get_scheduled(state: State<'_, AppState>) -> Result<Vec<ScheduledJob>, String> {
    shared::list_scheduled(&state.db.lock().unwrap())
}

#[command]
fn cancel_scheduled(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    shared::cancel_scheduled(&state.db.lock().unwrap(), id)
}

/// Kaçırılmış/hatalı işi hemen gönder.
#[command]
fn send_scheduled_now(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    shared::send_scheduled_now(&state.db.lock().unwrap(), id)
}

#[command]
fn clear_scheduled_history(state: State<'_, AppState>) -> Result<usize, String> {
    shared::clear_finished_scheduled(&state.db.lock().unwrap())
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

/// Menü çubuğundaki simgeden pencereyi geri getirir.
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn main() {
    let tray = SystemTray::new().with_menu(
        SystemTrayMenu::new()
            .add_item(CustomMenuItem::new("show", "Pencereyi Aç"))
            .add_native_item(SystemTrayMenuItem::Separator)
            .add_item(CustomMenuItem::new("quit", "Çıkış")),
    );

    tauri::Builder::default()
        .system_tray(tray)
        .on_system_tray_event(|app, event| match event {
            SystemTrayEvent::LeftClick { .. } | SystemTrayEvent::DoubleClick { .. } => {
                show_main_window(app);
            }
            SystemTrayEvent::MenuItemClick { id, .. } => match id.as_str() {
                "show" => show_main_window(app),
                "quit" => std::process::exit(0),
                _ => {}
            },
            _ => {}
        })
        // Pencereyi kapatmak uygulamayı öldürmesin: menü çubuğunda beklerken
        // zamanlanmış gönderimler çalışmaya devam eder.
        .on_window_event(|event| {
            if let WindowEvent::CloseRequested { api, .. } = event.event() {
                api.prevent_close();
                let _ = event.window().hide();
            }
        })
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
            // Zamanlanmış mesajlar: pencere kapalıyken (menü çubuğunda) de
            // çalışan arka plan turu.
            let sched_handle = app.handle();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(shared::SCHEDULE_POLL_SECS)).await;
                    let handle = sched_handle.clone();
                    let emit_handle = handle.clone();
                    let emit = move |name: &str, payload: serde_json::Value| {
                        let _ = emit_handle.emit_all(name, payload);
                    };
                    let state = handle.state::<AppState>();
                    shared::scheduler_tick(&state, emit).await;
                }
            });
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
            schedule_messages,
            get_scheduled,
            cancel_scheduled,
            send_scheduled_now,
            clear_scheduled_history,
            save_group,
            get_groups,
            delete_group
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
