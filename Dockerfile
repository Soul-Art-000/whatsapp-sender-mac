# ---------------------------------------------------------------------------
# Aşama 1: sunucu ikilisini derle (Tauri/WebKitGTK yok — yalnızca çekirdek).
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY frontend ./frontend
# Bağımlılıkları önce derle (katman önbelleği): kaynak değişince hepsi yeniden derlenmesin.
RUN cargo build --release --no-default-features --features server \
 && strip target/release/whatsapp-server

# ---------------------------------------------------------------------------
# Aşama 2: çalışma imajı — derlenmiş ikili + gömülü arayüz, başka hiçbir şey.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /data --shell /usr/sbin/nologin whatsapp \
 && mkdir -p /data/uploads \
 && chown -R whatsapp:whatsapp /data

COPY --from=build /src/target/release/whatsapp-server /usr/local/bin/whatsapp-server
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod 0755 /entrypoint.sh

ENV WA_BIND=0.0.0.0:8080 \
    WA_DATA_DIR=/data \
    WA_UPLOAD_DIR=/data/uploads

VOLUME /data
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/ >/dev/null || exit 1

# Gerekirse erişim anahtarını üretir, sonra sunucuyu çalıştırır.
ENTRYPOINT ["/entrypoint.sh"]
