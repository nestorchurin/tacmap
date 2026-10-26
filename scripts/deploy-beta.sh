#!/usr/bin/env bash
set -euo pipefail

# ==============================================================================
# TACMAP BETA DEPLOYMENT SCRIPT (STUB / READER COUNTER ONLY)
# Експериментальний радарний трекінг видалено та деактивовано.
# Цей скрипт оновлює лише статичну сторінку-заглушку та лічильник відвідувачів.
# ==============================================================================

PROXMOX_HOST="${PROXMOX_HOST:-192.168.1.100}"
PROXMOX_PASS="${PROXMOX_PASS:-}"
CT_ID="${CT_ID:-111}"

if [ -z "$PROXMOX_PASS" ]; then
  echo "ПОМИЛКА: Встановіть змінну оточення PROXMOX_PASS перед запуском."
  exit 1
fi

echo "=== [1/3] Збірка beta_counter ==="
cd /root/code/tacmap/backend
cargo build --release --bin beta_counter --quiet
echo "✓ Збірка успішна: target/release/beta_counter"

echo "=== [2/3] Доставка на CT $CT_ID ==="
cd /root/code/tacmap
sshpass -p "$PROXMOX_PASS" scp -o StrictHostKeyChecking=no \
  backend/target/release/beta_counter \
  root@"$PROXMOX_HOST":/tmp/beta_counter

sshpass -p "$PROXMOX_PASS" scp -o StrictHostKeyChecking=no \
  frontend_beta/index.html \
  root@"$PROXMOX_HOST":/tmp/beta_index.html

sshpass -p "$PROXMOX_PASS" ssh -o StrictHostKeyChecking=no root@"$PROXMOX_HOST" "
  pct push $CT_ID /tmp/beta_counter /opt/tacmap_beta/bin/beta_counter.new -perms 755
  pct exec $CT_ID -- mv -f /opt/tacmap_beta/bin/beta_counter.new /opt/tacmap_beta/bin/beta_counter
  pct push $CT_ID /tmp/beta_index.html /opt/tacmap_beta/frontend/index.html
  pct exec $CT_ID -- chown -R tacmap:tacmap /opt/tacmap_beta
  pct exec $CT_ID -- systemctl restart tacmap-beta
"

echo "=== [3/3] Перевірка статусу ==="
sshpass -p "$PROXMOX_PASS" ssh -o StrictHostKeyChecking=no root@"$PROXMOX_HOST" "
  pct exec $CT_ID -- systemctl is-active tacmap-beta
"
echo "=== ✓ ДЕПЛОЙ ЗАГЛУШКИ BETA ЗАВЕРШЕНО ==="
