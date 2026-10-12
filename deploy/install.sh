#!/usr/bin/env bash
# lively-oppenheimer sunucu kurulumu (Debian/Ubuntu, systemd).
# Kullanım: sudo bash deploy/install.sh [binary_yolu]
#   binary_yolu verilmezse ./target/release/whatsapp-server aranır.
set -euo pipefail

BIN="${1:-target/release/whatsapp-server}"
APP_DIR=/opt/whatsapp-sender
DATA_DIR=/var/lib/whatsapp-sender
ENV_FILE=/etc/whatsapp-sender.env

if [[ $EUID -ne 0 ]]; then echo "root olarak çalıştırın (sudo)." >&2; exit 1; fi
if [[ ! -f "$BIN" ]]; then echo "binary bulunamadı: $BIN" >&2; exit 1; fi

id -u whatsapp &>/dev/null || useradd --system --home "$DATA_DIR" --shell /usr/sbin/nologin whatsapp

install -d -o whatsapp -g whatsapp "$APP_DIR" "$DATA_DIR" "$DATA_DIR/uploads"
install -m 0755 "$BIN" "$APP_DIR/whatsapp-server"
chown whatsapp:whatsapp "$APP_DIR/whatsapp-server"

# Erişim anahtarı: yoksa üret (arayüz ?token=... ile açılır).
if [[ ! -f "$ENV_FILE" ]]; then
  TOKEN="$(head -c 24 /dev/urandom | base64 | tr -d '/+=' | head -c 24)"
  printf 'WA_TOKEN=%s\n' "$TOKEN" > "$ENV_FILE"
  chmod 600 "$ENV_FILE"
  echo "Erişim anahtarı üretildi: $TOKEN"
  echo "Arayüz adresi: http://<sunucu-ip>:8080/?token=$TOKEN"
fi

install -m 0644 "$(dirname "$0")/whatsapp-server.service" /etc/systemd/system/whatsapp-server.service
systemctl daemon-reload
systemctl enable --now whatsapp-server
systemctl --no-pager status whatsapp-server | head -20
