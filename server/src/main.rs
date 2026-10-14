//! Sunucu (headless) ikilisi: aynı çekirdeği HTTP API + WebSocket üzerinden sunar.
//!
//! Arayüz (`frontend/index.html`) ikilinin içine gömülüdür; tarayıcıdan
//! `http://<sunucu>:8080/` açılır. Tauri/WebKitGTK gerekmez, systemd ile 7/24
//! çalışır.
//!
//! Derleme: `cargo build --release --no-default-features --features server`
//! Ortam değişkenleri:
//!   WA_BIND        dinlenecek adres           (varsayılan 127.0.0.1:8080)
//!   WA_DATA_DIR    veritabanı + WhatsApp oturumu (varsayılan ./data)
//!   WA_UPLOAD_DIR  yüklenen dosyalar          (varsayılan <data>/uploads)
//!   WA_TOKEN       erişim anahtarı; boşsa kimlik doğrulama kapalı

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use whatsapp_sender::shared::{self, AppState, Contact};

/// Arayüz ikilinin içine gömülür: sunucuda ayrı bir statik dosya sunmak
/// (nginx/CDN) gerekmez.
const INDEX_HTML: &str = include_str!("../../frontend/index.html");

#[derive(Clone)]
struct Server {
    state: Arc<AppState>,
    events: broadcast::Sender<Value>,
    app_dir: Arc<PathBuf>,
    upload_dir: Arc<PathBuf>,
    token: Option<Arc<String>>,
    /// Son üretilen QR kodu: arayüz sunucu bağlandıktan sonra açılırsa kaçırmasın.
    last_qr: Arc<Mutex<Option<String>>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddContactArgs {
    name: String,
    phone: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendArgs {
    contacts: Vec<Contact>,
    message: String,
    #[serde(default)]
    attachment_paths: Vec<String>,
    #[serde(default)]
    validate: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleArgs {
    contacts: Vec<Contact>,
    message: String,
    #[serde(default)]
    attachment_paths: Vec<String>,
    #[serde(default = "default_true")]
    validate: bool,
    send_at: u64,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveGroupArgs {
    name: String,
    #[serde(default)]
    contact_ids: Vec<i64>,
}

// --------------------------------------------------------------- yardımcılar

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Yüklenen dosya adını güvenli bir dosya adına indirger (yol kaçışı yok).
/// Belge gönderiminde bu ad alıcıya görünür, o yüzden boşluk ve parantez korunur.
fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("dosya");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "dosya".to_string()
    } else {
        cleaned
    }
}

fn parse_args<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("Geçersiz istek: {}", e))
}

/// Tauri komut adlarıyla birebir aynı API. Arayüzdeki `invoke('x', args)`
/// çağrıları buraya düşer; dönen gövde JSON, hata gövdesi düz metindir.
async fn dispatch(srv: &Server, cmd: &str, args: Value) -> Result<Value, String> {
    match cmd {
        "get_contacts" => {
            let list = shared::get_contacts(&srv.state.db.lock().unwrap())?;
            Ok(json!(list))
        }
        "add_contact" => {
            let a: AddContactArgs = parse_args(args)?;
            shared::add_contact(&srv.state.db.lock().unwrap(), &a.name, &a.phone)?;
            Ok(Value::Null)
        }
        "remove_contact" => {
            let a: HashMap<String, String> = parse_args(args)?;
            let phone = a.get("phone").cloned().unwrap_or_default();
            shared::remove_contact(&srv.state.db.lock().unwrap(), &phone)?;
            Ok(Value::Null)
        }
        "clear_contacts" => {
            let deleted = shared::clear_contacts(&srv.state.db.lock().unwrap())?;
            Ok(json!(deleted))
        }
        "import_contacts_from_file" => {
            let a: HashMap<String, String> = parse_args(args)?;
            let path = PathBuf::from(a.get("path").cloned().unwrap_or_default());
            // Yalnızca sunucunun kendi yüklediği dosyalar okunur (yol kaçışına kapalı).
            let upload_dir = srv
                .upload_dir
                .canonicalize()
                .unwrap_or_else(|_| (*srv.upload_dir).clone());
            let real = path
                .canonicalize()
                .map_err(|e| format!("Dosya bulunamadı: {}", e))?;
            if !real.starts_with(&upload_dir) {
                return Err("Bu yola izin verilmiyor; dosyayı arayüzden yükleyin.".to_string());
            }
            let mut db = srv.state.db.lock().unwrap();
            let result = shared::import_contacts_from_file(&mut db, &real)?;
            Ok(json!(result))
        }
        "connect_whatsapp" => {
            // QR zaten üretildiyse (arayüz sonradan açıldı) onu tekrar göster;
            // yoksa bağlanmayı başlat.
            let pending_qr = srv.last_qr.lock().unwrap().clone();
            match pending_qr {
                Some(code) => {
                    let _ = srv
                        .events
                        .send(json!({ "event": "qr_code", "payload": code }));
                }
                None => spawn_connect(srv.clone()),
            }
            Ok(Value::Null)
        }
        "get_wa_status" => {
            let status = shared::wa_status(&srv.state.client.lock().unwrap());
            Ok(json!(status))
        }
        "disconnect_whatsapp" => {
            shared::disconnect_session(&srv.state).await?;
            Ok(Value::Null)
        }
        "logout_whatsapp" => logout(srv).await,
        "send_whatsapp_messages" => send(srv, parse_args(args)?).await,
        "stop_sending" => {
            srv.state.stop.store(true, Ordering::SeqCst);
            Ok(Value::Null)
        }
        "schedule_messages" => {
            let a: ScheduleArgs = parse_args(args)?;
            let id = shared::add_scheduled(
                &srv.state.db.lock().unwrap(),
                a.send_at,
                &a.message,
                &a.attachment_paths,
                &a.contacts,
                a.validate,
            )?;
            Ok(json!(id))
        }
        "get_scheduled" => {
            let list = shared::list_scheduled(&srv.state.db.lock().unwrap())?;
            Ok(json!(list))
        }
        "cancel_scheduled" => {
            let a: HashMap<String, i64> = parse_args(args)?;
            shared::cancel_scheduled(
                &srv.state.db.lock().unwrap(),
                a.get("id").copied().unwrap_or_default(),
            )?;
            Ok(Value::Null)
        }
        "send_scheduled_now" => {
            let a: HashMap<String, i64> = parse_args(args)?;
            shared::send_scheduled_now(
                &srv.state.db.lock().unwrap(),
                a.get("id").copied().unwrap_or_default(),
            )?;
            Ok(Value::Null)
        }
        "clear_scheduled_history" => {
            let n = shared::clear_finished_scheduled(&srv.state.db.lock().unwrap())?;
            Ok(json!(n))
        }
        "save_group" => {
            let a: SaveGroupArgs = parse_args(args)?;
            shared::save_group(&srv.state.db.lock().unwrap(), &a.name, &a.contact_ids)?;
            Ok(Value::Null)
        }
        "get_groups" => {
            let list = shared::get_groups(&srv.state.db.lock().unwrap())?;
            Ok(json!(list))
        }
        "delete_group" => {
            let a: HashMap<String, String> = parse_args(args)?;
            let name = a.get("name").cloned().unwrap_or_default();
            shared::delete_group(&srv.state.db.lock().unwrap(), &name)?;
            Ok(Value::Null)
        }
        other => Err(format!("Bilinmeyen komut: {}", other)),
    }
}

async fn logout(srv: &Server) -> Result<Value, String> {
    shared::logout_session(&srv.state, &srv.app_dir).await?;
    let _ = srv
        .events
        .send(json!({ "event": "whatsapp_logged_out", "payload": null }));
    Ok(Value::Null)
}

async fn send(srv: &Server, args: SendArgs) -> Result<Value, String> {
    let client = srv
        .state
        .client
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "WhatsApp bağlı değil! Lütfen önce QR kodu okutun.".to_string())?;

    // Aynı anda tek gönderim (zamanlanmış tur da bu bayrağı kullanır).
    if srv.state.busy.swap(true, Ordering::SeqCst) {
        return Err("Şu anda başka bir gönderim sürüyor. Bitmesini bekleyin.".to_string());
    }

    srv.state.stop.store(false, Ordering::SeqCst);
    let stop = srv.state.stop.clone();
    let tx = srv.events.clone();
    let emit = move |msg: String| {
        let _ = tx.send(json!({ "event": "send_status", "payload": msg }));
    };

    let result = shared::send_batch(
        client,
        &args.contacts,
        &args.message,
        &args.attachment_paths,
        args.validate,
        &stop,
        emit,
    )
    .await;
    srv.state.busy.store(false, Ordering::SeqCst);
    let summary = result?;

    let _ = srv
        .events
        .send(json!({ "event": "send_done", "payload": summary }));
    Ok(json!(summary))
}

/// Olayları WebSocket istemcilerine dağıtır; QR kodunu ayrıca saklar ki
/// arayüz açıldığında (sunucu çoktan bağlanmışsa) QR ekranda görünebilsin.
fn broadcaster(srv: &Server) -> impl Fn(&str, Value) + Clone + Send + Sync + 'static {
    let tx = srv.events.clone();
    let last_qr = srv.last_qr.clone();
    move |name: &str, payload: Value| {
        match name {
            "qr_code" => {
                if let Some(code) = payload.as_str() {
                    *last_qr.lock().unwrap() = Some(code.to_string());
                }
            }
            // Bağlantı kurulunca/logout olunca eski QR geçersizdir.
            "whatsapp_ready" | "whatsapp_logged_out" => *last_qr.lock().unwrap() = None,
            _ => {}
        }
        let _ = tx.send(json!({ "event": name, "payload": payload }));
    }
}

/// WhatsApp istemcisini arka planda başlatır (masaüstündeki `spawn_connect`).
fn spawn_connect(srv: Server) {
    if srv.state.client.lock().unwrap().is_some() {
        return;
    }
    if srv.state.connecting.swap(true, Ordering::SeqCst) {
        return;
    }

    tokio::spawn(async move {
        let emit = broadcaster(&srv);
        let state = srv.state.clone();
        let on_client = move |client: Arc<wa_rs::Client>| {
            *state.client.lock().unwrap() = Some(client);
        };

        if let Err(e) = shared::run_bot(&srv.app_dir, emit, on_client).await {
            let _ = srv
                .events
                .send(json!({ "event": "whatsapp_error", "payload": e }));
        }
        srv.state.connecting.store(false, Ordering::SeqCst);
        *srv.state.client.lock().unwrap() = None;
        let _ = srv.events.send(json!({
            "event": "whatsapp_disconnected",
            "payload": "Bağlantı kapandı."
        }));
    });
}

// -------------------------------------------------------------- HTTP uçları

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// Sağlık kontrolü: kimlik doğrulamasından muaf (Docker HEALTHCHECK buraya bakar).
async fn healthz() -> &'static str {
    "ok"
}

async fn api(State(srv): State<Server>, Path(cmd): Path<String>, body: Bytes) -> Response {
    let args: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    match dispatch(&srv, &cmd, args).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// Arayüzün yüklediği dosya (vCard, CSV, resim, belge) — sunucudaki geçici yol döner.
async fn upload(
    State(srv): State<Server>,
    Query(q): Query<HashMap<String, String>>,
    body: Bytes,
) -> Response {
    // Her yükleme kendi klasörüne: dosya adı (belge gönderiminde alıcıya görünür)
    // sayı ön eki almadan korunur, aynı adlı iki yükleme de birbirini ezmez.
    let name = safe_name(q.get("name").map(String::as_str).unwrap_or("dosya"));
    let dir = srv.upload_dir.join(now_nanos().to_string());
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Klasör oluşturulamadı: {}", e),
        )
            .into_response();
    }
    let path = dir.join(&name);
    match tokio::fs::write(&path, &body).await {
        Ok(_) => Json(path.to_string_lossy().to_string()).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Dosya yazılamadı: {}", e),
        )
            .into_response(),
    }
}

async fn ws(State(srv): State<Server>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| ws_loop(socket, srv))
}

async fn ws_loop(mut socket: WebSocket, srv: Server) {
    let mut rx = srv.events.subscribe();
    // Sunucu arayüz açılmadan bağlanmış olabilir: son QR'ı hemen gönder.
    // Not: değeri ara değişkene al — guard `await` boyunca yaşarsa future Send olmaz.
    let pending_qr = srv.last_qr.lock().unwrap().clone();
    if let Some(code) = pending_qr {
        let event = json!({ "event": "qr_code", "payload": code }).to_string();
        if socket.send(Message::Text(event.into())).await.is_err() {
            return;
        }
    }
    while let Ok(event) = rx.recv().await {
        if socket
            .send(Message::Text(event.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
    }
}

/// Tarayıcının başka bir adresten (ör. GitHub Pages arayüzü) bağlanabilmesi
/// için CORS. Token sorgu parametresiyle gittiğinden kimlik bilgisi paylaşımı
/// yok; bu yüzden yıldız yeterli.
async fn cors(req: Request, next: Next) -> Response {
    if req.method() == Method::OPTIONS {
        let mut res = StatusCode::NO_CONTENT.into_response();
        cors_headers(res.headers_mut());
        return res;
    }
    let mut res = next.run(req).await;
    cors_headers(res.headers_mut());
    res
}

fn cors_headers(headers: &mut axum::http::HeaderMap) {
    headers.insert("Access-Control-Allow-Origin", HeaderValue::from_static("*"));
    headers.insert(
        "Access-Control-Allow-Headers",
        HeaderValue::from_static("content-type, x-auth-token"),
    );
    headers.insert(
        "Access-Control-Allow-Methods",
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
}

/// `WA_TOKEN` tanımlıysa bütün uçlar (arayüz dahil) token ister:
/// `http://sunucu:8080/?token=...` ya da `X-Auth-Token` başlığı.
async fn require_token(State(srv): State<Server>, req: Request, next: Next) -> Response {
    if let Some(expected) = srv.token.as_deref() {
        // Arayüz sayfası ve sağlık ucu açıktır; veri uçları ve olay akışı anahtar ister.
        let path = req.uri().path();
        if path == "/" || path == "/healthz" {
            return next.run(req).await;
        }
        let from_query = req.uri().query().unwrap_or("").split('&').any(
            |kv| matches!(kv.split_once('='), Some(("token", value)) if value == expected.as_str()),
        );
        let from_header = req
            .headers()
            .get("x-auth-token")
            .and_then(|v| v.to_str().ok())
            == Some(expected.as_str());
        if !(from_query || from_header) {
            return (
                StatusCode::UNAUTHORIZED,
                "Yetkisiz: geçerli token gerekli.\n",
            )
                .into_response();
        }
    }
    next.run(req).await
}

#[tokio::main]
async fn main() {
    let bind: SocketAddr = std::env::var("WA_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
        .parse()
        .expect("WA_BIND geçersiz (örn. 0.0.0.0:8080)");

    let app_dir =
        PathBuf::from(std::env::var("WA_DATA_DIR").unwrap_or_else(|_| "data".to_string()));
    let upload_dir = PathBuf::from(
        std::env::var("WA_UPLOAD_DIR")
            .unwrap_or_else(|_| app_dir.join("uploads").to_string_lossy().to_string()),
    );
    std::fs::create_dir_all(&upload_dir).expect("yükleme klasörü oluşturulamadı");
    let token = std::env::var("WA_TOKEN").ok().filter(|t| !t.is_empty());

    let db = shared::init_db(&app_dir).expect("veritabanı açılamadı");
    let state = Arc::new(AppState::new(db));
    let (events, _) = broadcast::channel::<Value>(1024);

    let server = Server {
        state: state.clone(),
        events,
        app_dir: Arc::new(app_dir.clone()),
        upload_dir: Arc::new(upload_dir.clone()),
        token: token.map(Arc::new),
        last_qr: Arc::new(Mutex::new(None)),
    };

    println!("whatsapp-server dinliyor: http://{}", bind);
    println!("veri klasörü:  {}", app_dir.display());
    println!("yükleme klasörü: {}", upload_dir.display());
    if server.token.is_none() {
        println!("UYARI: WA_TOKEN tanımlı değil — arayüz kimlik doğrulaması kapalı.");
    }

    // Oturum daha önce eşleşmişse açılışta kendiliğinden bağlan.
    if app_dir.join("wa_bot.db").exists() {
        spawn_connect(server.clone());
    }

    // Zamanlanmış gönderimler: sunucu 7/24 açık olduğu için masaüstü
    // uygulaması kapalıyken de bu tur işleri yollar.
    {
        let srv = server.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(shared::SCHEDULE_POLL_SECS)).await;
                let emit = broadcaster(&srv);
                shared::scheduler_tick(&srv.state, emit).await;
            }
        });
    }

    let app = Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route("/ws", get(ws))
        .route("/api/upload", post(upload))
        .route("/api/{cmd}", post(api))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(
            server.clone(),
            require_token,
        ))
        .layer(middleware::from_fn(cors))
        .with_state(server);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .expect("port dinlenemedi");
    axum::serve(listener, app).await.expect("sunucu hata verdi");
}
