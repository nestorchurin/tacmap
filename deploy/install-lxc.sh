#!/usr/bin/env bash
# ==============================================================================
# TACMAP — Автоматичний установник для Proxmox / Debian 13 LXC
# ==============================================================================
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${BLUE}======================================================${NC}"
echo -e "${GREEN}   TACMAP — Розгортання в Proxmox / Debian 13 LXC    ${NC}"
echo -e "${BLUE}======================================================${NC}"

if [ "$EUID" -ne 0 ]; then
  echo -e "${RED}[!] Цей скрипт потрібно запускати від імені root (sudo)${NC}"
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# 1. Оновлення та встановлення системних залежностей
echo -e "\n${YELLOW}[1/6] Встановлення системних пакетів (Nginx, SQLite, SSL)...${NC}"
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  nginx \
  curl \
  ca-certificates \
  sqlite3 \
  libssl3 \
  tar \
  gzip

# 2. Створення системного користувача tacmap
echo -e "\n${YELLOW}[2/6] Налаштування користувача tacmap...${NC}"
if ! id -u tacmap &>/dev/null; then
  useradd -r -s /usr/sbin/nologin -d /opt/tacmap -m tacmap
  echo "Створено системного користувача tacmap."
else
  echo "Користувач tacmap уже існує."
fi

# 3. Підготовка директорій /opt/tacmap
echo -e "\n${YELLOW}[3/6] Розгортання структури каталогів /opt/tacmap...${NC}"
mkdir -p /opt/tacmap/bin
mkdir -p /opt/tacmap/frontend
mkdir -p /opt/tacmap/frontend_beta
mkdir -p /opt/tacmap/data
mkdir -p /opt/tacmap/config

# Копіювання фронтенду
if [ -d "${ROOT_DIR}/frontend" ]; then
  echo "Копіювання файлів фронтенду..."
  cp -r "${ROOT_DIR}/frontend/"* /opt/tacmap/frontend/
fi

# Копіювання бета-фронтенду
if [ -d "${ROOT_DIR}/frontend_beta" ]; then
  echo "Копіювання файлів бета-фронтенду..."
  cp -r "${ROOT_DIR}/frontend_beta/"* /opt/tacmap/frontend_beta/
fi

# Копіювання конфігурації
if [ ! -f /opt/tacmap/config/tacmap.env ]; then
  echo "Створення /opt/tacmap/config/tacmap.env..."
  cp "${SCRIPT_DIR}/tacmap.env" /opt/tacmap/config/tacmap.env
else
  echo "Файл конфігурації tacmap.env уже існує, пропускаємо перезапис."
fi

# Копіювання або компіляція бінарника
if [ -f "${ROOT_DIR}/backend/target/release/alerts-map-parser" ]; then
  echo "Копіювання зібраного release бінарника..."
  cp "${ROOT_DIR}/backend/target/release/alerts-map-parser" /opt/tacmap/bin/
  chmod +x /opt/tacmap/bin/alerts-map-parser
else
  echo -e "${YELLOW}[!] Готовий бінарник не знайдено в backend/target/release/.${NC}"
  if command -v cargo &>/dev/null; then
    echo "Компіляція release бінарника через Cargo..."
    (cd "${ROOT_DIR}/backend" && cargo build --release)
    cp "${ROOT_DIR}/backend/target/release/alerts-map-parser" /opt/tacmap/bin/
    chmod +x /opt/tacmap/bin/alerts-map-parser
  else
    echo -e "${RED}[!] Cargo не знайдено. Будь ласка, зберіть бінарник заздалегідь або встановіть Rust.${NC}"
    exit 1
  fi
fi

# Встановлення прав власності
chown -R tacmap:tacmap /opt/tacmap
chmod 750 /opt/tacmap/data

# 4. Встановлення та налаштування systemd сервісу
echo -e "\n${YELLOW}[4/6] Налаштування systemd сервісу (tacmap.service)...${NC}"
cp "${SCRIPT_DIR}/tacmap.service" /etc/systemd/system/tacmap.service
systemctl daemon-reload
systemctl enable tacmap.service
systemctl restart tacmap.service

# 5. Налаштування Nginx
echo -e "\n${YELLOW}[5/6] Налаштування Nginx Reverse Proxy та Rate-Limiting...${NC}"
cp "${SCRIPT_DIR}/nginx.conf" /etc/nginx/sites-available/tacmap
if [ -f /etc/nginx/sites-enabled/default ]; then
  rm -f /etc/nginx/sites-enabled/default
fi
ln -sf /etc/nginx/sites-available/tacmap /etc/nginx/sites-enabled/tacmap

nginx -t
systemctl enable nginx
systemctl restart nginx

# 6. Перевірка працездатності
echo -e "\n${YELLOW}[6/6] Перевірка статусу системи...${NC}"
sleep 2

if curl -s http://127.0.0.1:8080/api/health >/dev/null; then
  echo -e "${GREEN}[✓] Бекенд успішно запущено та відповідає на 127.0.0.1:8080${NC}"
else
  echo -e "${RED}[!] Попередження: бекенд ще ініціалізується. Перевірте: journalctl -u tacmap -f${NC}"
fi

IP_ADDR=$(hostname -I | awk '{print $1}')

echo -e "\n${BLUE}======================================================${NC}"
echo -e "${GREEN}   УСТАНОВКУ УСПІШНО ЗАВЕРШЕНО!                       ${NC}"
echo -e "${BLUE}======================================================${NC}"
echo -e "Мапа доступна за адресою:  ${YELLOW}http://${IP_ADDR}/${NC}"
echo -e "Стан сервісу:              systemctl status tacmap"
echo -e "Логи в реальному часі:     journalctl -u tacmap -f"
echo -e "Конфігурація:              /opt/tacmap/config/tacmap.env"
echo -e ""
echo -e "${BLUE}Для підключення HTTPS (Let's Encrypt SSL):${NC}"
echo -e "1. Вкажіть ваш домен у /etc/nginx/sites-available/tacmap (server_name)"
echo -e "2. Запустіть: certbot --nginx -d yourdomain.com"
echo -e "${BLUE}======================================================${NC}"
