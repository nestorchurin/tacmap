#!/usr/bin/env bash
set -euo pipefail

# ==============================================================================
# TACMAP RELEASE DEPLOYMENT SCRIPT (PRODUCTION ONLY)
# ==============================================================================

if [[ "${1:-}" != "--confirm-production" ]]; then
  echo "❌ ЗАХИСТ ВІД ВИПАДКОВОГО ОНОВЛЕННЯ РЕЛІЗУ!"
  echo "Ви намагаєтеся оновити бойовий Реліз (tacmap на порті 8080)."
  echo "Якщо ви впевнені, виконайте:"
  echo "  ./scripts/deploy-release.sh --confirm-production"
  exit 1
fi

PROXMOX_HOST="${PROXMOX_HOST:-192.168.1.100}"
PROXMOX_PASS="${PROXMOX_PASS:-}"
CT_ID="${CT_ID:-111}"

if [ -z "$PROXMOX_PASS" ]; then
  echo "ПОМИЛКА: Встановіть змінну оточення PROXMOX_PASS перед запуском."
  exit 1
fi

echo "=== [1/4] Тестування та збірка бінарника для Release ==="
export PATH="/root/.cargo/bin:$PATH"
cd /root/code/tacmap/backend
cargo test --quiet
cargo build --release --quiet
echo "✓ Збірка успішна: target/release/alerts-map-parser"

echo "=== [2/4] Копіювання файлів Release на Proxmox ==="
cd /root/code/tacmap
sshpass -p "$PROXMOX_PASS" scp -o StrictHostKeyChecking=no \
  backend/target/release/alerts-map-parser \
  frontend/index.html \
  frontend/style.css \
  frontend/app.js \
  root@"$PROXMOX_HOST":/tmp/

echo "=== [3/4] Доставка файлів у контейнер CT $CT_ID (/opt/tacmap) ==="
sshpass -p "$PROXMOX_PASS" ssh -o StrictHostKeyChecking=no root@"$PROXMOX_HOST" "
  pct push $CT_ID /tmp/alerts-map-parser /opt/tacmap/bin/alerts-map-parser -perms 755
  pct push $CT_ID /tmp/index.html /opt/tacmap/frontend/index.html
  pct push $CT_ID /tmp/style.css /opt/tacmap/frontend/style.css
  pct push $CT_ID /tmp/app.js /opt/tacmap/frontend/app.js
  pct exec $CT_ID -- chown -R 1000:1000 /opt/tacmap/frontend
"

echo "=== [4/4] Перезапуск сервісу tacmap (Release) ==="
sshpass -p "$PROXMOX_PASS" ssh -o StrictHostKeyChecking=no root@"$PROXMOX_HOST" "
  pct exec $CT_ID -- systemctl restart tacmap
  pct exec $CT_ID -- systemctl is-active tacmap
"

echo "=== ✓ ДЕПЛОЙ RELEASE УСПІШНО ЗАВЕРШЕНО ==="
echo "Release доступний за адресою: https://tacmap.yunexal.com/"
