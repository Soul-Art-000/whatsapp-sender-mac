# Sunucuda çalıştırma — Docker, GitHub Pages arayüzü, systemd

Aynı çekirdek üç biçimde çalışır:

| Biçim | Ne | Komut |
|---|---|---|
| Masaüstü | Tauri penceresi (macOS/Windows) | `cargo build --release` |
| Sunucu ikilisi | HTTP API + WebSocket, systemd ile 7/24 | `cargo build --release -p whatsapp-server` |
| Docker | Yukarıdakinin konteyneri, her şey hazır | `docker compose up -d` |

Sunucu hedefi **Tauri/WebKitGTK kullanmaz**, arayüz (`frontend/index.html`)
ikilinin içine gömülüdür: sunucuda ayrıca nginx/CDN kurmak gerekmez.

---

## VDS'e kurulum — önerilen akış

Backend'i bir VDS'te (Ubuntu/Debian) Docker ile çalıştırıp arayüzü GitHub Pages'ten
kullanma. Dışarıya yalnızca HTTPS açılır; uygulama portu dış dünyaya kapatılır.

```bash
# 1) Docker
curl -fsSL https://get.docker.com | sh

# 2) Uygulamayı başlat (yalnızca localhost'a bağlı, dışarıya kapalı)
#    (a) Hazır imaj — GHCR paketini bir kez "Public" yaptıysanız:
docker run -d --name whatsapp-sender --restart unless-stopped \
  -p 127.0.0.1:8080:8080 -v wa-data:/data \
  ghcr.io/soul-art-000/whatsapp-sender-mac:latest

#    (b) Paket private kaldıysa ya da kendiniz derlemek isterseniz
#        (repo public olduğu için kimlik bilgisi gerekmez; VDS'te ~10 dk derler, 2 GB+ RAM):
git clone https://github.com/Soul-Art-000/whatsapp-sender-mac.git
cd whatsapp-sender-mac && docker compose up -d --build

# 3) Erişim anahtarını öğren
docker exec whatsapp-sender cat /data/token

# 4) HTTPS (Pages arayüzü için şart — tarayıcı https sayfadan http isteğini engeller)
sudo apt-get install -y caddy
sudo tee /etc/caddy/Caddyfile >/dev/null <<'EOF'
wa.ornek.com {
    reverse_proxy 127.0.0.1:8080 {
        transport http {
            read_timeout 3600s
        }
    }
}
EOF
sudo systemctl reload caddy
```

DNS'te `wa.ornek.com` için VDS IP'sine bir A kaydı verin; Caddy sertifikayı
(Let's Encrypt) kendisi alır ve yeniler.

Sonra arayüz:

```
https://soul-art-000.github.io/whatsapp-sender-mac/?api=https://wa.ornek.com&token=ANAHTAR
```

QR görünür → telefondan WhatsApp → Bağlı cihazlar → Cihaz bağla.

**GHCR paketini public yapmak:** GitHub → profil → Packages → `whatsapp-sender-mac`
→ Package settings → Change visibility → Public. (Token'ınızda `write:packages`
yetkisi yoksa bu ayar yalnızca arayüzden yapılabilir.) Public yapmak istemiyorsanız
yukarıdaki (b) yolunu veya `docker login ghcr.io -u Soul-Art-000` kullanın.

**Domain yoksa / Pages istemiyorsanız:** arayüzü doğrudan
`http://<VDS-IP>:8080/?token=ANAHTAR` ile açın (bu durumda `-p 8080:8080` yapın ve
güvenlik duvarında portu açın). Bu, HTTPS gerektirmediği için en hızlı yoldur ama
token'ı ağ üzerinde düz metin taşır; kalıcı kullanımda Caddy'li kurulum tercih edin.

**Not:** VDS'in IP'si sabittir; WhatsApp bunu normal bir cihaz gibi görür. Uzun
gönderimler sırasında gönderim boyunca HTTP isteği açık kalır — proxy'de
`read_timeout`/`proxy_read_timeout` değerini yükseltmeyi unutmayın.

---

## 1) Docker (en kolay)

Yerelde imajı derleyip çalıştırmak:

```bash
docker compose up -d --build
docker logs -f whatsapp-sender     # arayüz adresi ve erişim anahtarı burada
```

Hazır imajı çekmek (GitHub Actions her push'ta GHCR'a yayınlar):

```bash
docker run -d --name whatsapp-sender \
  -p 8080:8080 -v wa-data:/data \
  ghcr.io/soul-art-000/whatsapp-sender-mac:latest
docker exec whatsapp-sender cat /data/token
```

- İlk açılışta erişim anahtarı `/data/token` içine üretilir, konteyner yeniden
  başlasa da değişmez; `docker logs` çıktısında da yazılır.
- Kişiler ve WhatsApp oturumu `wa-data` isimli kalıcı birimde durur.
- Kendi anahtarınızı vermek için: `-e WA_TOKEN=...`

Tarayıcı: `http://<sunucu-ip>:8080/?token=<anahtar>` → QR görünür, telefonunuzdan
WhatsApp → Bağlı cihazlar → Cihaz bağla ile okutun.

## 2) GitHub Pages arayüzü (uzaktan erişim)

Arayüz statik tek dosyadır ve Pages'te barındırılabilir:

```
https://soul-art-000.github.io/whatsapp-sender-mac/?api=https://wa.ornek.com&token=ANAHTAR
```

- `?api=` verilmezse arayüz aynı origin'i (kendi sunucusunu) kullanır; Pages'te
  açıldığında bir kez sunucu adresi sorulur ve tarayıcıda saklanır.
- Backend CORS destekler (`Access-Control-Allow-Origin: *`); anahtar sorgu
  parametresiyle gittiği için kimlik bilgisi paylaşımı yoktur.

**Şart:** Pages `https://` üzerinden açıldığı için backend de **https** olmalı —
tarayıcılar https sayfadan http isteğini (mixed content) engeller. Önüne TLS
koymanın en kısa yolu Caddy:

```caddy
wa.ornek.com {
    reverse_proxy 127.0.0.1:8080 {
        transport http {
            read_timeout 3600s
        }
    }
}
```

Caddy sertifikayı (Let's Encrypt) kendisi alır. Alternatifler: nginx + certbot,
Cloudflare Tunnel. Sadece kendi bilgisayarınızdan kullanacaksanız arayüzü
`http://sunucu:8080/` üzerinden açın, Pages'e gerek yoktur.

## 3) systemd ile ikili kurulumu (Docker istemeyenler için)

```bash
sudo apt-get update && sudo apt-get install -y build-essential pkg-config
# Rust yoksa: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
git clone https://github.com/Soul-Art-000/whatsapp-sender-mac.git
cd whatsapp-sender-mac
cargo build --release -p whatsapp-server
sudo bash deploy/install.sh target/release/whatsapp-server
```

Kurulum; `whatsapp` kullanıcısını açar, ikiliyi `/opt/whatsapp-sender`'a kopyalar,
veriyi `/var/lib/whatsapp-sender`'da tutar, `WA_TOKEN` üretip `/etc/whatsapp-sender.env`
(yetki 600) içine yazar ve servisi başlatır:

```bash
systemctl status whatsapp-server
journalctl -u whatsapp-server -f
```

## Ortam değişkenleri

| Değişken | Varsayılan | Açıklama |
|---|---|---|
| `WA_BIND` | `127.0.0.1:8080` | Dinlenecek adres (konteynerde `0.0.0.0:8080`). |
| `WA_DATA_DIR` | `./data` | `contacts.sqlite` + WhatsApp oturumu (`wa_bot.db`). |
| `WA_UPLOAD_DIR` | `<data>/uploads` | Arayüzden yüklenen vCard/CSV/resimler. |
| `WA_TOKEN` | boş | Boşsa kimlik doğrulama kapalı (yalnızca localhost için). |

## API

Arayüz Tauri komut adlarını çağırır; sunucuda aynı adlar POST uçlarına düşer.

```
POST /api/get_contacts              -> [{id,name,phone}]
POST /api/add_contact               {name, phone}
POST /api/remove_contact            {phone}
POST /api/clear_contacts            -> silinen kişi sayısı
POST /api/import_contacts_from_file {path}      (yalnızca yükleme klasörü)
POST /api/upload?name=dosya.vcf     (ham gövde) -> sunucudaki geçici yol
POST /api/connect_whatsapp
POST /api/get_wa_status             -> "offline" | "connecting" | "connected"
POST /api/disconnect_whatsapp
POST /api/logout_whatsapp
POST /api/send_whatsapp_messages    {contacts, message, imagePaths, validate}
POST /api/stop_sending
POST /api/save_group | get_groups | delete_group
GET  /ws                            olay akışı (qr_code, send_status, send_done, ...)
```

Anahtar `?token=...` parametresi ya da `X-Auth-Token` başlığı ile verilir.

## Notlar ve tuzaklar

- **Uzun gönderimler**: `/api/send_whatsapp_messages` gönderim bitene kadar açık
  kalır (yüzlerce kişide saatler). Önüne nginx koyarsanız
  `proxy_read_timeout 3600s;` verin — varsayılan 60 sn isteği koparır ve arayüz
  "kalanlar" listesini kaybeder. Caddy'de yukarıdaki `read_timeout` ayarı gerekir.
- **Tek örnek**: Aynı `WA_DATA_DIR`'ı iki süreç açmamalı; WhatsApp oturumu tek
  istemciye aittir (Docker'da bunu `container_name` + tek servis garantiler).
- **Yedek**: `contacts.sqlite` kişileri ve grupları tutar. `wa_bot.db` oturumdur,
  kopyalanabilir ama aynı anda iki makinede açılmamalı.
- **Çıkış**: "Oturumu kapat" yerel oturumu siler (QR yeniden okutulur); telefondaki
  kaydı kullanıcı WhatsApp → Bağlı cihazlar üzerinden kaldırır.
- **Ban riski**: Hız sınırları masaüstüyle aynıdır (4-9 sn arası, 25 kişide mola,
  mesaj başına 30 sn üst sınır, gönderim öncesi numara doğrulama).
- **Wi-Fi/asılı QR**: QR yalnızca `Event::PairingQrCode` geldiğinde çizilir;
  sunucu loglarında hata varsa çıkış yapıp yeniden bağlanın.
