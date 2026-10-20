# LightCraft Cloud: результат етапу 3

Дата перевірки: 2026-10-20. Зміни виконано лише в середовищі Codex.
Підключень до Oracle Cloud, змін DNS, firewall, Nginx Proxy Manager, Immich або
інших наявних сервісів не було. Попередні виправлення LC-01/02/03 збережено.

Реалізовано робочий незалежний backend у [cloud/](../cloud/README.md), а не
лише проєкт або Compose-шаблон. Основні перевірки етапу пройдено. Це основа
для наступної розробки; розгортання на OCI залишається окремою дією після дозволу.

## Реалізація

- Окремий Rust workspace: axum/tokio REST API, SQLx, PostgreSQL 17, міграція схеми.
  Залежності сервера не потрапляють до desktop/WASM workspace.
- Незалежний Keycloak realm, Authorization Code + PKCE S256, RS256 access tokens,
  перевірка issuer/audience/exp/nbf/iat/sub/type та scopes. Власник визначається
  парою `(iss, sub)`. Ресурси іншого власника повертають 404.
- Бібліотеки, реєстрація фотографій, основні метадані, одноразове потокове
  завантаження та отримання оригіналів JPEG/RAW без перекодування.
- Серверні UUID для шляхів, SHA-256 і довжина, незмінний JSON manifest,
  перевірка цілісності перед отриманням. Повторний PUT не перезаписує оригінал.
- SQL intent і резервування квоти перед передаванням; fsync файлу та директорії,
  атомарне перейменування, manifest і фінальна SQL-транзакція до успішної відповіді.
  Після збою завершені файли з pending-записами узгоджуються під час запуску.
- Operation IDs, revisions, tombstones для скасувань та транзакційний журнал
  `changes` готують основу синхронізації. Публічний sync feed ще не реалізовано.
- Окремі PostgreSQL бази/ролі для каталогу та Keycloak. API не має DDL-доступу
  чи права змінювати історію міграцій; міграції запускає окремий контейнер.
- Compose: власні мережі, постійні томи, ліміти CPU/RAM/PID, ротація логів,
  read-only/non-root API, файлові секрети поза Git. Базова конфігурація не публікує
  жодного порту; локальний override відкриває лише loopback API та Keycloak.
  PostgreSQL не має host port в обох конфігураціях.
- Підготовлено підключення до наявного NPM: API `lightcraft-api:8080`,
  авторизація `lightcraft-auth:8080`, окремі HTTPS-імена через наявний 443.
  NPM і сертифікати фактично не змінювалися.

Під час інтеграції виправлено конфігурацію Keycloak: окремий default scope
`lightcraft-identity` явно додає стабільний `sub`. Справжній токен без цього
claim відхиляється API; обов'язкові перевірки JWT не послаблювалися.

## Працюючі endpoints

Усі `/v1` потребують Bearer access token. Читання потребує `lightcraft:read`,
запис — `lightcraft:write`. Контракт: [OpenAPI](../cloud/openapi.json).

| Метод | Шлях | Призначення |
|---|---|---|
| GET | `/health/live` | Живий процес, без авторизації |
| GET | `/health/ready` | Готовність PostgreSQL, сховища та ключів OIDC |
| GET | `/v1/me` | Поточний користувач і scopes |
| POST, GET | `/v1/libraries` | Створення та список власних бібліотек |
| GET | `/v1/libraries/{id}` | Власна бібліотека |
| POST, GET | `/v1/libraries/{id}/photos` | Реєстрація та список фотографій |
| GET | `/v1/photos/{id}` | Метадані, стан, revision, SHA-256 і розмір |
| PUT | `/v1/photos/{id}/original` | Повне передавання оригіналу з перевіркою |
| GET | `/v1/photos/{id}/original` | Побайтово незмінний оригінал |
| POST | `/v1/photos/{id}/verify` | Повторна перевірка оригіналу та manifest |
| DELETE | `/v1/photos/{id}/upload` | Скасування pending; готовий оригінал не видаляє |

Разом 13 пар «метод + шлях». Списки мають UUID-pagination та обмеження 1–100.
Помилки повертаються як JSON без SQL чи секретів; ліміти, хибні параметри,
чужі ресурси, недостатні scopes, неправильні токени та пошкодження розрізняються.

## Реальні перевірки

Середовище: Debian 13 x86_64, Rust 1.95.0, Docker 28.4, Compose 2.40.3.
Ubuntu 24.04 та нативне ARM64 обладнання в Codex не використовувалися.

| Перевірка | Результат |
|---|---|
| Backend `cargo fmt` | PASS |
| Backend `cargo clippy --locked --all-targets -- -D warnings` | PASS |
| Backend `cargo test --locked` | 14/14: authentication 4, storage 8, validation 2 |
| Python local setup | 1/1; повторна ініціалізація зберігає секрети та налаштування |
| Compose config, базовий і локальний | PASS; PostgreSQL без host port |
| Docker amd64 API build із вихідного коду | PASS, контейнер запущено |
| Оптимізований Keycloak image build | PASS, контейнер запущено |
| Реальні HTTP/OIDC/PostgreSQL E2E, amd64 | 8/8, 165.645 s |
| Додаткова перевірка OIDC після уточнення identity fixture | 1/1, 40.770 s |
| Rust cross-build `aarch64-unknown-linux-musl` | PASS, статичний aarch64 ELF |
| Docker ARM64 API image | PASS, побудовано з cross-built binary |
| ARM64 API та мігратор під QEMU 7.2 | Версія `0.1.0 aarch64`; E2E 8/8, 169.451 s |
| ARM64 manifests PostgreSQL та Keycloak | `linux/arm64` наявний в обох pinned images |
| Основний застосунок `cargo xtask ci --summary-md` | Усі 8 кроків PASS; 191 пропуск |

E2E використовує справжній Keycloak, Authorization Code + PKCE та refresh rotation,
справжній PostgreSQL і HTTP API. Перевірено авторизацію, scopes, неправильну audience,
ID-token замість access token, відсутність sub, підміну підпису, двох незалежних
власників, ідемпотентність, JPEG та процедурний DNG/TIFF, метадані, SHA-256,
неправильні довжину/тип/hash, фактичний обрив TCP під час PUT і повторну спробу,
квоти, PostgreSQL привілеї, міграції та порядок changes.

Обидва E2E-прогони зупинили й заново створили контейнери PostgreSQL, Keycloak
та API з тими самими томами. Після цього запис каталогу, авторизація та оригінал
зі збіжним SHA-256 залишилися доступними. Окремо відтворено стан «файл уже
збережено, фінальна SQL-транзакція ще pending»: запуск повернув його в ready.
ARM64-прогін починався з нових томів, тому перевірено також bootstrap баз та
імпорт realm без ручного виправлення наявних даних.

У ARM64-прогоні саме API та мігратор виконувалися як ARM64 під QEMU.
PostgreSQL і Keycloak працювали як amd64; їхню ARM64-сумісність перевірено
за manifests, а не нативним запуском. GitHub workflow для `ubuntu-24.04` та
`ubuntu-24.04-arm` додано, але GitHub Actions у цій сесії не запускався.

У першій спробі CI основного застосунку був timeout на наявному MCP-тесті
`export_progress_and_cancel` під паралельним навантаженням збірок.
Він пройшов окремо; повний повтор `RUST_TEST_THREADS=1 cargo xtask ci --summary-md`
пройшов усі кроки: fmt, clippy, heif, test, parity, layers, assets, wasm.
191 пропуск: corpus 137, GPU 25, fonts 20, model 8, system 1.
Це не підтверджує перевірку відсутніх реальних RAW-корпусів чи GPU-сценаріїв.
Код desktop/engine/catalog на цьому етапі не змінювався.

Відтворення перевірок описано в [cloud README](../cloud/README.md#build-and-validation).
Локальний запуск і E2E призначені для ізольованих тестових даних, оскільки E2E
створює користувачів і відтворює контейнери. Паролі, токени, тестові облікові
дані, build artifacts і томи в Git не додаються.

## Створені та змінені файли

Створено 35 файлів:

```text
.gitattributes
.github/workflows/cloud.yml
cloud/.dockerignore
cloud/.env.example
cloud/.gitignore
cloud/Cargo.lock
cloud/Cargo.toml
cloud/Dockerfile
cloud/Dockerfile.keycloak
cloud/Dockerfile.prebuilt
cloud/Dockerfile.prebuilt.dockerignore
cloud/README.md
cloud/compose.local.yaml
cloud/compose.yaml
cloud/docker/init-db.sh
cloud/docker/realm-lightcraft.json
cloud/docker/runtime/.keep
cloud/migrations/0001_cloud.sql
cloud/openapi.json
cloud/scripts/init-local.py
cloud/src/api.rs
cloud/src/auth.rs
cloud/src/config.rs
cloud/src/db.rs
cloud/src/error.rs
cloud/src/lib.rs
cloud/src/main.rs
cloud/src/models.rs
cloud/src/storage.rs
cloud/tests/authentication.rs
cloud/tests/e2e.py
cloud/tests/storage.rs
cloud/tests/test_local_setup.py
cloud/tests/validation.rs
docs/cloud-stage3.md
```

Оновлено 5 файлів документації:

```text
README.md
ROADMAP.md
docs/architecture.md
docs/gaps.md
docs/parity-checklist.md
```

## Межі етапу та наступна робота

Повне передавання RAW/JPEG через API вже працює. Перевірка сигнатури контейнера
не є RAW-декодером; метадані надає клієнт. E2E використовує синтетичні файли,
а не весь набір камер. Після обриву PUT повторюється з нульового байта.

Для етапу 4 залишаються:

1. Підключення desktop-клієнта: фонове завантаження RAW/JPEG, локальна черга/outbox,
   кеш, контроль повторів та робота офлайн.
2. Відновлюване передавання: upload sessions, chunks/offset, перевірка частин,
   retry/backoff та фіналізація без повторного завантаження всього оригіналу.
3. Обмежений за ресурсами worker для EXIF, мініатюр і прев'ю; версії результатів,
   захищені endpoints та локальне кешування без завантаження RAW.

Повна двостороння синхронізація правок, альбомів, конфліктів і видалень потребує
окремого клієнтського протоколу та наступних міграцій. Ці можливості не оголошені
готовими на підставі самої наявності changes/revisions.

Перед реальним розгортанням потрібні окремо дозволені перевірки ресурсів OCI,
HTTPS через NPM, постійного адміністратора Keycloak/MFA, резервного копіювання
та відновлення. Постійні Docker-томи не замінюють backup. Квота API і запас вільного
місця не створюють hard quota для PostgreSQL/WAL та інших сервісів на спільному
диску. Сервіс підтримує один процес-письменник на том originals.
