#!/bin/sh
# WA_TOKEN verilmemişse ilk açılışta üretir ve /data/token içinde saklar;
# böylece konteyner "her şey hazır" başlar ve token yeniden başlatmada değişmez.
set -e

DATA_DIR="${WA_DATA_DIR:-/data}"

if [ -z "${WA_TOKEN:-}" ]; then
  TOKEN_FILE="$DATA_DIR/token"
  if [ ! -s "$TOKEN_FILE" ]; then
    head -c 32 /dev/urandom | base64 | tr -d '/+=' | cut -c1-24 > "$TOKEN_FILE"
    chmod 600 "$TOKEN_FILE"
  fi
  WA_TOKEN="$(cat "$TOKEN_FILE")"
  export WA_TOKEN
  echo "-------------------------------------------------------------"
  echo "WhatsApp Sender sunucusu başlıyor"
  echo "  arayüz : http://<sunucu-adresi>:8080/?token=$WA_TOKEN"
  echo "  token  : $WA_TOKEN  (yoksa: docker exec <konteyner> cat /data/token)"
  echo "-------------------------------------------------------------"
fi

exec whatsapp-server
