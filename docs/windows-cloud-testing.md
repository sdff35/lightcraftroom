# Тестування LightCraft Cloud на Windows x64

Це portable-збірка **наявної десктопної програми**, із Cloud Library, Settings → Cloud,
входом Keycloak OIDC/PKCE через системний браузер та локальною бібліотекою/редактором.
Cloud входить до звичайної native-збірки: окремий feature flag не потрібний.
Ціль — Windows 10/11 x64 (`x86_64-pc-windows-msvc`), не Windows ARM64 чи x86.

## Поточний статус

Windows executable ще потрібно зібрати. У Codex на момент підготовки цих інструкцій
був Linux executable; Windows runner відсутній, а запити GitHub Actions/Releases
повертали `Forbidden`. Наявність завантажуваного Windows-артефакту з Cloud не підтверджена.
Наведений нижче workflow підготовлений для запуску в GitHub, але його Windows-результат
не слід вважати перевіреним, доки конкретний run не стане зеленим.

## Отримання через GitHub Actions

Перед першим push відкрийте <https://github.com/sdff35/lightcraftroom/actions> і,
якщо Actions у fork вимкнено, увімкніть їх. Це дозволить одразу запустити workflow
від push, навіть якщо він ще не потрапив у default branch.

1. Перенесіть **усю поточну гілку** з Codex у `sdff35/lightcraftroom` через PR/експорт
   змін. Вона має містити Cloud-коміт `ecf3b02a` та
   `.github/workflows/windows-x64-cloud.yml`. Одного workflow поверх старого коду недостатньо.
   Якщо ця гілка вже є у вашому локальному Git-клоні, її можна завантажити окремо,
   не змінюючи `main` чи `release`:

   ```powershell
   git merge-base --is-ancestor ecf3b02a HEAD
   if ($LASTEXITCODE -ne 0) { throw 'Потрібна гілка з Cloud-комітом ecf3b02a' }
   if (-not (Test-Path .github/workflows/windows-x64-cloud.yml)) { throw 'Немає нового workflow' }
   git push origin HEAD:codex/windows-cloud
   ```

   Якщо ви завантажили наданий разом зі звітом `lightcraft-cloud-windows-preview.bundle`,
   він містить повну гілку `work` з історією й усіма потрібними комітами. У PowerShell
   в папці із цим файлом (потрібен Git for Windows):

   ```powershell
   git clone -b work .\lightcraft-cloud-windows-preview.bundle LightCraftCloudBuild
   Set-Location .\LightCraftCloudBuild
   git remote set-url origin https://github.com/sdff35/lightcraftroom.git
   git push origin HEAD:codex/windows-cloud
   ```

   Увійдіть у GitHub через стандартний Git Credential Manager у власному браузері,
   якщо Git цього потребує. Не передавайте облікові дані в чат. Bundle містить
   вихідний код, **не готовий `.exe`**; push запускає Windows-збірку.

2. Відкрийте <https://github.com/sdff35/lightcraftroom/actions>.
   Push у `codex/windows-cloud`, `work` або `main` зі змінами відповідних файлів запускає
   **Windows x64 Cloud preview** автоматично.
3. Для ручного запуску: **Actions → Windows x64 Cloud preview → Run workflow → потрібна
   гілка → Run workflow**. GitHub показує ручну кнопку після появи workflow в default
   branch; до цього використовуйте запуск від push у зазначену гілку.
4. Дочекайтеся зеленого job `desktop`. Перевірте SHA коміту у run: він повинен включати
   Cloud та цей workflow. Збірка запускається на `windows-2022`, Rust 1.95.0, з pinned
   craft-fonts, `Cargo.lock` та статичною CRT. Сертифікати підпису й серверні секрети
   не потрібні. Workflow має лише `contents: read` і нічого не розгортає.
5. Унизу run, у **Artifacts**, завантажте **LightCraft-Cloud-windows-x64-<повний SHA>**.
   Артефакти зберігаються 14 днів; для завантаження потрібен вхід у GitHub.
   Не плутайте його з артефактом `...-evidence-...`, який містить звіт і скриншоти.
6. Розпакуйте ZIP артефакту GitHub. Усередині будуть:

   - `lightcraft-cloud-windows-x64-<короткий SHA>.zip` — portable-програма;
   - `SHA256SUMS.txt` — SHA-256 цього ZIP;
   - `BUILD-INFO.json` — точний commit, ціль та SHA-256 executable.

Перевірка у PowerShell, відкритому в цій папці:

```powershell
$line = (Get-Content .\SHA256SUMS.txt -Raw).Trim() -split '\s+', 2
$actual = (Get-FileHash -LiteralPath $line[1] -Algorithm SHA256).Hash
if ($actual -ne $line[0]) { throw 'SHA-256 ZIP не збігається' }
Expand-Archive -LiteralPath $line[1] -DestinationPath .\LightCraftCloudTest
$exe = Get-ChildItem .\LightCraftCloudTest -Recurse -Filter lightcraft.exe | Select-Object -First 1
$info = Get-Content (Join-Path $exe.DirectoryName 'BUILD-INFO.json') -Raw | ConvertFrom-Json
if ((Get-FileHash $exe.FullName -Algorithm SHA256).Hash -ne $info.executable_sha256) {
  throw 'SHA-256 executable не збігається'
}
$info
```

## Запуск без змішування з основною бібліотекою

Не запускайте програму безпосередньо із ZIP. Розпакована папка має бути доступна для запису.
Rust, Docker, PostgreSQL і Keycloak **не потрібні на Windows для відкриття GUI**.

У тому ж PowerShell після перевірки архіву:

```powershell
$testRoot = Join-Path $env:LOCALAPPDATA 'LightCraftCloudTest'
$env:LIGHTCRAFT_CONFIG_DIR = Join-Path $testRoot 'Config'
$library = Join-Path $testRoot 'Library'
& $exe.FullName --library $library --no-demo
```

Повторюйте ці команди для наступних запусків, щоб використовувати той самий тестовий
профіль. Звичайний подвійний клік використовує стандартні налаштування LightCraft,
а не `$testRoot`. Portable означає запуск без інсталятора, а не автоматичну ізоляцію
налаштувань. Токени зберігаються у Windows Credential Manager і прив'язані до сервера;
вони не переносяться разом із папкою й не записуються у `ui.json`.

Preview не підписаний Authenticode, тому SmartScreen може показати попередження.
Звірте репозиторій, SHA коміту й контрольні суми; не вимикайте Defender/SmartScreen
глобально. Якщо політика Windows забороняє непідписані програми, потрібна дозволена
підписана збірка. Адміністративні права для portable GUI не потрібні.

## Що перевірити вручну

1. Відкрийте локальну бібліотеку, імпортуйте **тестову копію** JPEG/RAW і перевірте редактор.
2. Натисніть Cloud у верхній панелі або Cloud Library у навігації. Має з'явитися
   порожній стан із пропозицією підключення; через **← Local Library** поверніться назад.
3. Відкрийте **Settings → Cloud**. Перевірте поле адреси, **Save & test connection**
   і **Sign in with browser**. До успішного підключення кнопка входу вимкнена — це очікувано.
4. Для реального входу потрібен **окремо запущений LightCraft Backend**, включно з
   `GET /v1/config`, PostgreSQL, сховищем та Keycloak. Immich URL не підходить.
   Якщо Backend ще не розгорнуто, доступні лише GUI/локальна бібліотека та перевірка
   помилок; ця збірка не створює і не налаштовує сервер.
5. Введіть кореневу HTTPS-адресу API, наприклад `https://cloud.example.com`, без `/v1`,
   пароля, query чи токена. Перевірте з'єднання. Issuer Keycloak має бути доступним
   з Windows через HTTPS із довіреним сертифікатом. Для цього ручного тесту використовуйте
   HTTPS issuer: штатний desktop opener відхиляє HTTP-посилання.
6. Натисніть **Sign in with browser**, увійдіть на сторінці Keycloak у браузері та
   поверніться до LightCraft. Клієнт `lightcraft-desktop`: public client, Standard Flow,
   PKCE S256; точний дозволений redirect — `http://127.0.0.1:47861/callback`.
   Це локальний callback тільки на час входу: відкривати порт 47861 у OCI/роутері не потрібно.
   Пароль вводиться тільки на сторінці Keycloak. Параметри realm описані в `cloud/README.md`.
7. Створіть бібліотеку, завантажте невеликий JPEG та RAW, отримайте оригінал у **новий**
   локальний файл. Зіставте `Get-FileHash` обох файлів. Перезапустіть програму з тим самим
   профілем і перевірте відновлення входу, потім **Sign out** та відсутність автовходу.
8. Перевірте зрозумілу помилку для некоректної адреси/недоступної мережі. RAW може мати
   тільки вбудоване прев'ю; **Load image** наразі отримує оригінал. Автосинхронізація,
   відновлювані передачі й серверні прев'ю залишаються поза цим етапом.

Якщо програма не відкривається, збережіть лог із `$testRoot\Config\logs\lightcraft.log`.
Для діагностики рендерингу можна перед запуском встановити `$env:LIGHTCRAFT_GPU = '0'`.
Не додавайте у звіти токени, паролі чи повний callback URL. Якщо вхід завис — закрийте
інший сеанс входу LightCraft і перевірте зайнятість локального порту:

```powershell
Get-NetTCPConnection -LocalPort 47861 -ErrorAction SilentlyContinue
```

## Межі автоматичної перевірки

Workflow має завершитися помилкою до публікації ZIP, якщо не проходить хоча б одна перевірка:

- Cloud client: адреси, callback, integrity, no-clobber, redirects, помилки авторизації;
- Cloud UI: вузький/широкий екран, Settings, повернення до локальних правок, некоректні адреси;
- Windows browser command: передавання повного URL системному обробнику без shell;
- запуск саме зібраного `lightcraft.exe`, перехід у Cloud, Settings, наявність кнопки входу,
  повернення до Local Library та завершення процесу через власний control channel;
- PE machine `0x8664` (x64), GUI subsystem, контрольні суми пакета.

Скріншоти створюються Headless renderer усередині запущеного desktop-процесу, не є
перевіркою Windows compositor чи фізичного введення. Workflow **не виконує онлайн-вхід**
і не перевіряє Windows Credential Manager із реальним акаунтом. Це ручні кроки вище.
Попередній повний Cloud E2E пройшов у Linux Codex з локальними Backend/Keycloak;
його не слід називати Windows E2E.
Під час підготовки workflow перевірено `actionlint`, синтаксис його PowerShell-кроків
та скрипта пакування. Новий `cloud-smoke.py` успішно виконав навігаційний сценарій
на наявному Linux desktop executable під Xvfb. Windows build/run лишається перевіркою
GitHub runner після завантаження гілки.

## Якщо потрібен GitHub Release

Для особистого тестування достатньо Actions artifact; workflow не створює Release.
Після зеленого run maintainer може створити **draft prerelease** у
<https://github.com/sdff35/lightcraftroom/releases/new>, вибрати **той самий commit**,
окремий тег на кшталт `cloud-preview-<короткий SHA>` й додати portable ZIP,
`SHA256SUMS.txt` та `BUILD-INFO.json` із цього run. Після перевірки можна опублікувати
prerelease вручну; це не розгортання Backend. Звичайний upstream Release або старий
`0.9.1` не доводить наявності Cloud — перевіряйте commit.

Існуючий `release.yml` призначений для загальних підписаних релізів і може вимагати
налаштованого `release` environment. Він не потрібний для цього preview.
