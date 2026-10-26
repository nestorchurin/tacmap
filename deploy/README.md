# 🛡️ TACMAP — Керівництво з розгортання в Proxmox / Debian 13 LXC

Це керівництво описує процес встановлення та публікації радарного агрегатора **TACMAP** у захищеному LXC-контейнері з **Debian 13 (Trixie)** під керуванням **Proxmox VE**.

---

## 1. Рекомендовані параметри LXC контейнера

У панелі керування Proxmox створіть LXC контейнер із такими параметрами:

* **OS Template:** `debian-13-standard` (або `debian-12` з наступним оновленням)
* **CPU:** 1–2 ядра (vCPU)
* **RAM:** 512 МБ (достатньо 256–512 МБ, сервіс у релізі споживає ~25–40 МБ ОЗП)
* **Swap:** 512 МБ
* **Disk:** 4–8 ГБ (база точності SQLite займає ~100–300 МБ)
* **Network:** Статична IP або DHCP (мостовий режим `vmbr0`)
* **Unprivileged container:** `Yes` (підтримується повна ізоляція)

---

## 2. Швидке встановлення в 1 команду

1. Зайдіть у консоль створеного контейнера (або через SSH під `root`):
   ```bash
   # Оновіть списки пакетів та git
   apt-get update && apt-get install -y git

   # Склонуйте репозиторій або скопіюйте теку з проєктом
   git clone https://github.com/your-username/AlertsMapParser.git /root/tacmap-src
   cd /root/tacmap-src
   ```

2. Запустіть автоматичний інсталятор:
   ```bash
   ./deploy/install-lxc.sh
   ```

Скрипт автоматично:
- Встановить `nginx`, `sqlite3`, `libssl3`, `ca-certificates`;
- Створить системного користувача `tacmap` із правами доступу;
- Розгорне файли у `/opt/tacmap/` (`bin`, `frontend`, `data`, `config`);
- Сконфігурує та запустить ізольований **systemd-сервіс** (`tacmap.service`);
- Налаштує **Nginx** із реверс-проксі, WebSockets, rate-limiting та захистом від сканерів.

---

## 3. Закриття «дитячих болячок» та захист API для публікації

У цій збірці реалізовано комплексний захист для безпечної роботи в публічному інтернеті:

### 🔒 1. Мережева ізоляція (Localhost Binding)
* Rust-бекенд прив'язаний виключно до `127.0.0.1:8080` (через параметр `HOST=127.0.0.1` у `/opt/tacmap/config/tacmap.env`).
* Порт `8080` **не доступний ззовні** — увесь трафік іде суто через Nginx reverse proxy.

### 🛑 2. Захист API від скрапінгу та DoS (Rate Limiting)
У файлі `/etc/nginx/sites-available/tacmap` налаштовано зони лімітів:
* **`/api/`**: обмеження до **10 запитів/сек** на одну IP-адресу (`burst=20 nodelay`). Якщо сторонній бот спробує масово долбити API — він отримає `429 Too Many Requests`.
* **`/ws/geo`**: обмеження до **3 спроб підключення/сек** для захисту від флуду сокетами.
* **Статичні файли**: ліміт **30 запитів/сек**.

### 🚫 3. Блокування сканерів та шпигунських ботів
* Автоматичне блокування запитів до прихованих файлів: `/\.(?!well-known)`, `.env`, `.git`.
* Блокування доступу до баз даних: `*.sqlite`, `*.db`, `*.bak`.
* Блокування автоматичних пошукачів вразливостей (`wp-login`, `phpmyadmin`, `admin`, `actuator` тощо) з миттєвим `403 Forbidden` без навантаження на Rust.

### 🛡️ 4. Заголовки безпеки (HTTP Security Headers)
Nginx та Rust додають обов'язкові інтернет-заголовки:
* `X-Content-Type-Options: nosniff` (захист від MIME-плутанини);
* `X-Frame-Options: SAMEORIGIN` (захист від клікджекінгу);
* `Referrer-Policy: strict-origin-when-cross-origin`;
* `Permissions-Policy: geolocation=(), camera=(), microphone=()`.

### 🔕 5. Вимкнення відладкових ендпоінтів
* Ендпоінт `/api/debug/outliers` вимкнено в релізі (`ENABLE_DEBUG=false`).
* Валідація параметрів: діапазон вибірки теплової карти обмежено максимум 720 годинами (30 днів), що унеможливлює зависання запитів.

### 🧱 6. Systemd Hardening (Пісочниця)
Сервіс `tacmap.service` працює з максимально строгими обмеженнями ядра:
* `NoNewPrivileges=true`
* `ProtectSystem=full` (коренева система файлів тільки для читання)
* `ProtectHome=true`
* `PrivateTmp=true`
* Запис дозволено тільки у `/opt/tacmap/data` (де лежить `accuracy.db`).

---

## 4. Підключення власного домену та безкоштовного SSL (HTTPS)

Для того, щоб сервіс відкривався через безпечний `https://`:

1. Направте DNS-запис (A-запис) вашого домену на публічну IP-адресу вашого сервера/роутера (і прокиньте порти 80 та 443 на IP LXC-контейнера).
2. Відредагуйте конфіг Nginx:
   ```bash
   nano /etc/nginx/sites-available/tacmap
   ```
   Змініть рядок:
   ```nginx
   server_name tacmap.yourdomain.com;
   ```
3. Перевірте та перезавантажте Nginx:
   ```bash
   nginx -t && systemctl reload nginx
   ```
4. Отримайте сертифікат Let's Encrypt через Certbot:
   ```bash
   apt-get install -y certbot python3-certbot-nginx
   certbot --nginx -d tacmap.yourdomain.com
   ```
Certbot автоматично пропише SSL-сертифікати, налаштує автооновлення та увімкне редирект з HTTP на HTTPS.

---

## 5. Корисні команди для обслуговування

* **Перевірити стан бекенда:**
  ```bash
  systemctl status tacmap
  ```
* **Переглянути живі логи роботи:**
  ```bash
  journalctl -u tacmap -f -n 50
  ```
* **Перезапустити сервіс після зміни конфігу:**
  ```bash
  systemctl restart tacmap
  ```
* **Змінити параметри (затримка, джерела):**
  ```bash
  nano /opt/tacmap/config/tacmap.env
  systemctl restart tacmap
  ```
* **Оновити фронтенд або бінарник:**
  ```bash
  # Скопіювати нові файли у /opt/tacmap/frontend/
  # Перезапустити: systemctl restart tacmap
  ```
