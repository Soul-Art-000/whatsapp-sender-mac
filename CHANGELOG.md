# Değişiklik Günlüğü

Bu dosya [Keep a Changelog](https://keepachangelog.com/tr/1.1.0/) düzenini ve
[Semantic Versioning](https://semver.org/lang/tr/) sürümlemeyi izler.

## [0.2.0] - 2026-10-12

### Added
- **Sunucu modu (headless)**: aynı çekirdek `whatsapp-server` ikilisiyle HTTP API +
  WebSocket üzerinden sunulur; arayüz ikilinin içine gömülüdür, Tauri/WebKitGTK
  gerekmez. `cargo build --release --no-default-features --features server`
- **Docker imajı**: çok aşamalı `Dockerfile` + `docker-compose.yml`; ilk açılışta erişim
  anahtarı üretilir; GitHub Actions imajı GHCR'a yayınlar ve duman testi çalıştırır.
- **GitHub Pages arayüzü**: statik arayüz Pages'te barındırılır, `?api=https://sunucu`
  ile uzaktaki backend'e bağlanır (CORS destekli).
- Erişim anahtarı (`WA_TOKEN`) ve yalnızca yükleme klasöründen okuma (yol kaçışı engeli).

### Changed
- Tauri'den bağımsız mantık `src/shared.rs`'e taşındı; masaüstü ve sunucu tek çekirdeği
  paylaşır (gönderim/anti-ban/import mantığı bölünmez).
- `tauri` bağımlılığı `desktop` özelliği arkasına alındı; sunucu hedefi Linux'ta
  WebKitGTK olmadan derlenir.

## [Unreleased]

### Added
- İlk sürüm öncesi çalışmalar.
