# Технический аудит обновления Atlas — 7 октября 2026

## 1. Вывод и границы исследования

**Текущий Windows updater проверяет подлинность скачанного пакета, но не обеспечивает транзакционную установку. Сбой после начала замены файлов может оставить Atlas неработоспособным. Последняя рабочая версия до подтверждения новой не сохраняется.**

Механизм обновления в рамках аудита не изменялся. Исследована рабочая ветка `fix/network-recovery-release`, HEAD `9d54431b80574553904572ac0214526d6ea6d2c0` плюс существующие незакоммиченные исправления восстановления сети и подписок. Это кандидат 2.3.2-alpha.1, а не доказательство поведения всех ранее выпущенных сборок. PR #56 закрыт, публикации не выполнялись.

Источники: код приложения, hooks NSIS, scripts/workflows, исходники фактически закреплённого Rust `tauri-plugin-updater 3.0.0-alpha.1` и используемого им `reqwest 0.13.5` в локальном Cargo registry, два предоставленных диагностических отчёта. Разрушительные сценарии установки, reboot, потери питания и антивирусных блокировок на рабочем компьютере не воспроизводились. Вероятности ниже — качественная оценка условий отказа, не статистика инцидентов.

Обнаружено расхождение окружения: package-lock закрепляет CLI и JS updater **3.0.0-alpha.1**, а локальный node_modules содержит CLI **2.11.4**, JS updater **2.11.0**. Существующий `src-tauri/target/release/nsis/x64/installer.nsi` относится к **1.0.0-beta.17.1**. Его нельзя использовать как доказательство точного шаблона нового установщика. CI выполняет `npm ci`; локальную сборку нужно привести к lockfile и повторить проверки. Прохождение текущего frontend build само по себе этого не подтверждает.

## 2. Компоненты и ответственность

| Компонент | Роль |
|---|---|
| `src/main.tsx`, `src/updatePolling.ts` | Проверка при запуске, каждые 6 часов и вручную; download/install; UI статусы |
| `src-tauri/tauri.conf.json` | Endpoint latest.json, minisign pubkey, NSIS perMachine, quiet mode |
| `src-tauri/src/lib.rs` | Регистрация стандартного updater, frontend_diagnostic, сетевое состояние приложения |
| Rust plugin `src/commands.rs`, `src/updater.rs`, `src/config.rs` | HTTP, выбор платформы, SemVer, RAM-буфер, подпись, temp EXE, ShellExecuteW |
| `src-tauri/installer-hooks.nsh` | Запуск нового maintenance до замены, установка службы после замены, uninstall |
| `src-tauri/src/maintenance_main.rs`, `maintenance.rs` | Независимое от CEF завершение процессов и восстановление сети |
| `network_guard.rs`, `windows.rs`, `service.rs`, `job.rs`, `session_cleanup.rs` | WFP/TUN/маршруты, прежний proxy, SCM, владение процессами и очистка |
| `.github/workflows/release.yml`, `scripts/publish-verified-update.ps1` | Сборка, подпись, тесты, draft release, assets, перевод в latest |
| `scripts/stamp-release-version.cjs`, `verify-installer.cjs` | Уникальные версии, проверка minisign и вычисление SHA-256 на стороне выпуска |
| `scripts/test-packaged-installer.ps1`, `test-connected-upgrade.ps1`, `test-ui-startup.ps1` | Проверки пакета и ограниченная установка на disposable Windows |
| `scripts/installer-launcher/Program.cs`, `installer/Payload.cs`, `installer/manifest.json` | Отдельный офлайн-комплект: сборка частей, размер/SHA-256, запуск NSIS |
| `incident_history.rs` | Ограниченная история событий и redaction, но не журнал транзакции установки |

Не следует смешивать две цепочки: встроенный updater скачивает EXE и проверяет minisign; офлайн launcher склеивает parts и проверяет встроенные размер/SHA-256. Гарантии офлайн launcher автоматически не распространяются на встроенное обновление. Пользовательские незакоммиченные `scripts/update-local.ps1` и `LOCAL-UPDATE.md` не являются штатным release pipeline и не изменялись.

## 3. Фактический lifecycle

1. React запускает `startUpdatePolling`; текущая проверка защищена локальным флагом running. Проверки пропускаются, пока updateBusy установлен.
2. `check({timeout:15000})` запрашивает `https://github.com/snaps-creator/atlas/releases/latest/download/latest.json`. Desktop **не вызывает GitHub Releases API**. API используется скриптом публикации.
3. Reqwest следует обычной redirect policy. Проверяются HTTPS/TLS, читается JSON, версия сравнивается SemVer. Выбирается платформенный URL/подпись, сначала возможный `windows-x86_64-nsis`, затем `windows-x86_64`.
4. При нажатии «Обновить» metadata запрашиваются повторно; выбранная версия может обновиться относительно баннера.
5. `latest.download(callback)` получает весь EXE в `Vec<u8>` внутри процесса Atlas. Переданный в check timeout **не наследуется**: Update создаётся с timeout=None; download options в UI отсутствуют.
6. Событие Finished посылается после чтения тела, **до** minisign verification. UI уже пишет «Установка», хотя подпись ещё проверяется. Однако `await download` завершится успешно только после проверки: запуск неподписанных байтов этой последовательностью не разрешается.
7. Подпись проверяется встроенным pubkey. Если trusted comment содержит версию, plugin сопоставляет её metadata. `requireSignedVersion` не включён, legacy-подписи без версии допускаются. Отдельных ожидаемых size/SHA-256 в latest.json нет.
8. `latest.install({restartAfterInstall:true})` пишет пакет в случайный temp-каталог. Передача updater идёт уже после полной загрузки/проверки. Независимый установщик не продолжает этот HTTP download.
9. Plugin вызывает ShellExecuteW с `/S /UPDATE /R ...`. При ошибке запуска возвращает ошибку; при результате >32 немедленно делает `std::process::exit(0)`. Это подтверждает запуск, не успешную установку. Обычный teardown приложения/Rust Drop не выполняется.
10. Новый установщик получает необходимые повышенные права. PREINSTALL извлекает **новый** AtlasMaintenance.exe в PLUGINSDIR и синхронно вызывает `--prepare-install`.
11. Maintenance проверяет путь SCM службы, прекращает desktop-процессы по точному пути, пытается остановить службу, завершает принадлежащие установке Core/Xray/Service, повторно проверяет workers и SCM. Далее ждёт освобождения TUN, очищает доказанно принадлежащие Atlas маршруты/legacy WFP, восстанавливает сохранённые proxy-настройки загруженных профилей и очищает DNS cache.
12. Ненулевой maintenance result останавливает установку **до** замены файлов, но приложение/VPN к этому моменту уже могли завершиться. Ошибка записывается в `%TEMP%/atlas-install-recovery.log`.
13. При успехе NSIS устанавливает в текущий `$INSTDIR`. POSTINSTALL копирует Maintenance и Atlas.Service.exe, запускает новый Atlas.exe `--install-service`; затем отключает автоматические действия восстановления SCM. Служба проверяет совпадение hash UI/Service.
14. Ожидается перезапуск через `/R`. Нет прикладного ACK от нового Atlas, ожидания стабильного UI/SCM handshake, отката бинарных файлов или recovery journal. Точный restart-код нового NSIS необходимо сохранить из воспроизводимой CI-сборки; локальный generated template устарел.

Отдельно: double-click установленного пакета перенаправляется hook в `/P /UPDATE /R`, чтобы не запускать старый uninstaller до нового recovery. Это полезная защита, а не транзакционный rollback.

## 4. Что говорят предоставленные логи

В `atlas-diagnostics-1791356183.txt` найдены updater события:

| Unix time | Событие |
|---|---|
| 1791355303 / 5304 | check_started / available |
| 1791355311 | download_started |
| 1791355373 | install_started — через 62 секунды |
| 1791355393 / 5394 | check_started / current — приблизительно через 20 секунд после handoff |

Это совместимо с загрузкой, передачей пакета установщику и последующим запуском приложения. **Это не health-check установки**: current лишь означает отсутствие более новой версии по metadata. После этого отчёт содержит CleanupError сетевого восстановления. В другом отчёте есть только check_started/current. Отчёты относятся к различным сетевым окружениям; объединять их в непрерывную историю одной машины нельзя.

В переданных данных нет полного журнала NSIS, версии каждого заменённого файла, exit code инсталлятора и transaction ID. Поэтому точную причину каждого случая «приложение перестало запускаться» установить по этим отчётам нельзя. Подтверждены конкретные уязвимые переходы в коде, но не воспроизведена конкретная аварийная замена из пользовательского инцидента.

## 5. VPN, proxy, DNS и сеть

- Atlas не передаёт updater явный proxy 127.0.0.1:17890. Но plugin собирается с system-proxy и не получает no_proxy. Он может использовать обнаруженный системный/environment proxy, включая оставшийся localhost proxy. Это условная зависимость, а не доказательство, что все updater-запросы идут через localhost.
- Обычные TCP/DNS запросы updater следуют системным маршрутам. При активном TUN маршрут GitHub/CDN определяется политикой Atlas; он может идти через VPN или напрямую. Отсутствие явного HTTP proxy не означает обход TUN. Даже no_proxy() не исключит TUN.
- При отключении VPN/смене сервера может оборваться уже существующее TCP/TLS-соединение. Изменение исходного IP не восстанавливает его. Нет прикладного resume/retry download.
- GitHub frontend, metadata asset и EXE asset могут попадать на разные CDN. Доступность одного адреса не доказывает доступность остальных. 403/429/5xx, DNS/TLS failures сейчас переходят в общий error, без классификации и backoff.
- Штатная последовательность **не** закрывает Atlas, пока пакет скачивается. Если пользователь сам завершит Atlas во время download, скачивание обрывается в процессе, старая установка ещё не изменена.
- После handoff интернет для локальной замены уже не нужен. Делать доступность публичного сайта обязательным условием установки полностью подготовленного пакета не следует: отсутствие интернета не должно запрещать офлайн-восстановление.
- Смерть desktop/service может закрыть owned workers, освободить Wintun и динамические WFP handles. Это не равно доказательству восстановления всех маршрутов/DNS/proxy. Для этого нужен maintenance. DNS cache flush сам по себе не проверяет рабочий resolver или отсутствие оставшегося DNS-hijack.
- Существующий maintenance проверяет отдельные результаты последовательно, а не получает единый устойчивый baseline/epoch. Первый wait error остаётся в failures даже если последующие действия уже освободили ресурс. На активном VPN этот путь значительно более нагружен, чем на выключенном.
- Выгруженный пользовательский hive с proxy marker блокирует восстановление. Это сознательный fail-closed сценарий; он требует понятного сообщения, а не принудительного удаления чужих proxy-настроек.

## 6. Реестр недостатков

Вероятность указана условно: «высокая при X» означает воспроизводимое следствие X, а не частоту среди пользователей. P0 — потеря рабочей установки; P1 — серьёзный отказ/блокировка обновления; P2 — диагностический/UX/воспроизводимость риск.

| ID | Проблема → причина | Последствия | Вероятность | Критичность | Решение |
|---|---|---|---|---|---|
| U01 | Неатомарная установка → существующий INSTDIR, последовательные изменения, нет сохраняемой версии/commit record | Частичная смесь EXE/DLL/core/resources при disk-full, lock, crash или reboot; старая версия уже затронута | Высокая при сбое в окне замены | P0 | Неизменяемые version directories, подготовка до остановки, durable switch record, прежняя версия до health ACK |
| U02 | Postinstall может отказать после замены → CopyFiles/регистрация SCM происходят поздно; ошибки отдельных CopyFiles/sc не проверяются полностью | UI обновлён, service старый/недоступный; Abort не возвращает прежние файлы/SCM | Средняя, высокая при ACL/AV/SCM ошибке | P0 | Верификация payload, контролируемое SCM switch, откат всей версии и SCM состояния |
| U03 | Нет startup health/rollback → installer launch принят за конечный handoff, нового ACK нет | CEF failure/немедленный crash/new DB incompatibility не обнаруживаются updater | Условная, при дефекте кандидата высокая | P0 | Authenticated nonce/version/PID health, окно стабильности, локальный rollback |
| U04 | Загрузка без deadline → check timeout обнулён в Update, download без options | Долгое зависание при чёрной дыре/частичном ответе | Средняя в нестабильной сети | P1 | Connect/read-idle/overall deadlines отдельно; отмена и bounded retry |
| U05 | Нет прикладного retry/resume → весь download в памяти | Сбой 99% требует начать заново; ручной retry снова зависит от маршрута | Высокая при disconnect | P1 | Durable .part, ETag/If-Range/Content-Range, полная итоговая верификация; backoff+jitter |
| U06 | Неявная зависимость от сети Atlas → system-proxy defaults и системный TUN | Proxy unavailable/DNS/CDN failure при VPN и его выключении | Зависит от политики сети | P1 | Явная политика proxy, route-aware повтор, независимая staging-фаза; не обещать обход TUN через no_proxy |
| U07 | Handoff без supervision → ShellExecuteW >32, затем process::exit | Тихий installer Abort/exit не возвращается UI, пользователю кажется, что Atlas исчез | Высокая при ошибке после запуска | P1 | Независимый supervisor с process handles, exit codes, durable result и отдельным UI |
| U08 | Нет доказанной глобальной блокировки update → React state/ref не machine-wide lock; hook не удерживает mutex до health | Два installer или повторный запуск Atlas могут пересечься с заменой | Средняя при ручном/авто запуске | P1 | ACL-защищённый machine mutex, transaction ID и блокировка новых sessions до завершения |
| U09 | Очистка не подтверждается последним единым baseline → wait/cleanup stages и сохранённые errors; нет update/session lease | Ложный Abort после фактического освобождения либо новый process/session между inspection и overwrite | Средняя при активном VPN/медленной остановке | P1 | Final idempotent inspection, блокировка reconnect/start, повторная проверка непосредственно перед switch |
| U10 | Нет durable recovery после reboot → temp/процессы не являются журналом | Непонятно, нужно продолжать, откатывать или запускать предыдущую версию | Высокая при reboot во время замены | P0 | Persisted state machine, recovery при старте launcher/службы, crash-injection на каждом переходе |
| U11 | Неполная целостность metadata/temp handoff → есть minisign bytes, нет обязательной signed version, явного expected size/hash и повторной проверки после записи | Legacy replay metadata, слабая диагностика повреждения; нет явной гарантии тех же bytes в файле при handoff | Низкая для обычного сбоя; условная при tampering | P1 | Signed manifest version/platform/size/hash, проверка durable staged bytes непосредственно перед использованием; ACL и file handles |
| U12 | Неограниченный RAM download и неполная уборка → Vec, tempdir.keep(), нет Atlas quota/GC; UI не закрывает все Update resources | Память/диск расходуются при больших пакетах и повторах | Средняя; офлайн пакет в репозитории около 189 MB, размер текущего release отдельно не подтверждён | P2 | Streaming на диск, лимиты, free-space preflight, GC только завершённых transaction directories |
| U13 | UX не отражает факт проверки → Finished до подписи; общий install error, нет restart/success/rollback states | Непонятный этап, неподтверждённый успех и неверное ожидание | Высокая | P2 | Явные состояния с привязкой к durable transaction |
| U14 | Неполные логи → UI events и общий temp maintenance log | Нельзя связать версию, HTTP, shutdown, installer, новые процессы и итог | Высокая при расследовании | P1 | Единый redacted update journal с transaction ID и стадиями |
| U15 | Локальные зависимости/артефакты не соответствуют lock → устаревшие node_modules и generated NSIS | Локальный тест не доказывает поведение сборки CI; ошибочные выводы о restart/template | Подтверждено локально | P2 | npm ci, чистый build, сохранить generated script/version/hash как evidence |
| U16 | Тест «connected upgrade» не равен production VPN → TUN auto-route=false, DNS=false, desktop identity fixture | Не проверяются download через VPN/CDN, реальный shutdown DNS/WFP, пользовательский restart, rollback | Подтверждено по тесту | P1 | Дополнить disposable VM полноценной network/update fault matrix |

Reqwest имеет ограниченные низкоуровневые повторы для безопасных transport failures. Это не пользовательская политика retry/backoff для 429/503/оборванного тела и не resume. Формулировка «вообще никаких повторов внутри HTTP-библиотеки» была бы неточной.

## 7. Гонки и сценарии неработоспособной установки

1. **Два install:** UI не проверяет `updateBusy.current` как синхронный входной lock, а state обновляется асинхронно. Даже исправление этого не исключит внешний EXE/другую сессию Windows. Нужна блокировка на машине. Наличие и поведение стандартной NSIS-защиты в новом generated template ещё требуется подтвердить; независимо от неё нет блокировки, охватывающей все собственные компоненты и health.
2. **Старый Atlas снова запускается:** maintenance завершил обнаруженные processes и сделал второй snapshot, но не запрещает новый запуск между snapshot и заменой. Single-instance desktop не является installation lock.
3. **Owner death / service stop / maintenance:** plugin делает exit, observers уже освобождают сеть, maintenance одновременно инспектирует и прекращает службу. Требуется идемпотентность и финальная сверка; это не доказательство ошибки конкретного Windows API в каждом запуске.
4. **SCM switch после новых файлов:** новые UI/resources уже на месте, CopyFiles Service или Create/ChangeService/ACL fails. Служба дополнительно отвергнет различающиеся hash UI/Service. Предыдущего набора нет.
5. **Power loss/AV/disk full посередине extraction:** часть установки новая, часть старая. Наличие подписи EXE не делает файловую замену транзакционной.
6. **Успешный installer, нерабочая новая версия:** CEF/DLL startup, доступ к профилю или migration fail; rollback бинарников/данных отсутствует. SQLite settings backups — не backup программы и не протокол совместимости схем.
7. **Отказ до extraction:** package download/signature/UAC failure обычно сохраняет старые файлы. Отказ PREINSTALL сохраняет файлы, но не гарантирует работающий desktop/VPN после уже выполненного завершения. Нельзя одинаково описывать оба случая как «ничего не изменилось».

## 8. Что следует сохранить

- GitHub Releases как хранилище, draft до полной загрузки assets и контролируемое переключение latest.
- Minisign с закреплённым ключом; цифровая подпись здесь уже есть. Authenticode EXE — отдельное дополнение для publisher identity/Windows UX, не замена minisign.
- Полная загрузка и проверка до остановки VPN. Нет необходимости ломать этот правильный порядок ради отдельного сетевого updater.
- Новый независимый от CEF maintenance, exact-path/PID-handle ownership, проверка SCM path; не применять blanket kill/delete всех адаптеров/маршрутов.
- Abort до замены при неподтверждённой очистке; точное восстановление только принадлежащего Atlas proxy.
- Проверка hash Atlas.exe/Atlas.Service.exe и ограниченные права SCM.
- Существующие package/signature/UI/service tests и offline installer; расширять их, а не подменять mocks.

## 9. Рекомендуемая архитектура

**Минимальная надёжная цель: сохранить источник обновлений и собственный maintenance, заменить неатомарную in-place фазу на независимую транзакцию активации версии.** Один backup поверх старой папки недостаточен: restore тоже может оборваться, потерять права или столкнуться с locks.

Предлагаемая структура на одном локальном NTFS volume:

```
Atlas/
  Launcher.exe               стабильная точка запуска и восстановления
  Updater.exe                небольшой supervisor без CEF
  versions/<version-build>/  неизменяемый, проверенный набор UI/core/resources
  state/current.json         generation, active, previous, pending transaction
  transactions/<id>/         signed manifest, journal, staged package, result
```

1. Downloader читает подписанный manifest с version/build/platform/size/SHA-256/signature. Потоковая загрузка в защищённый cache `.part`, ограниченные retries. Возобновление только при совпадении immutable asset identity/ETag; изменившийся asset загружается заново.
2. Проверить size/hash/signature, записать файл, flush, повторно проверить staged bytes. Ни URL, ни путь из низкопривилегированного UI не должны разрешать elevated updater читать/исполнять произвольный файл. Защитить от symlink/reparse/path traversal и подмены.
3. Supervisor получает machine mutex и transaction ID, подтверждает handshake UI. Полностью извлечь новую версию в отдельный version directory и проверить manifest всех файлов **пока старая версия ещё работает**. Проверить объём диска/ACL/AV locks. Это сокращает окно без VPN.
4. Journal `Prepared` после устойчивой записи. Старый desktop запрещает reconnect, новые launcher invocations видят maintenance lock. Выполнить graceful shutdown с deadline; новый maintenance обеспечивает bounded recovery неисправного старого клиента.
5. Финальная сверка SCM/owned PIDs/TUN/WFP/routes/owned proxy. Зафиксировать `Quiesced`. Не требовать публичного интернета: базовая чистота системы отделена от доступности сети провайдера.
6. Journal `SwitchIntent` включает previous/candidate, прежнюю регистрацию службы и поколение pointer. Изменить SCM path на проверенный version-specific Service, затем активный pointer с replace-and-flush протоколом. Это **не одна транзакция с SCM**: crash между ними должен восстанавливаться journal reconciler. Никогда не полагаться на два rename как на неделимое действие.
7. Запустить desktop под исходным обычным пользователем, а не под случайной UAC-учётной записью. Supervisor остаётся независимо от desktop, получает PID handle и authenticated nonce/version/build ACK. Health: UI event loop, readable settings, IPC handshake нужной версии, owned core smoke без обязательного публичного сайта; период стабильности, например 30 секунд, как настраиваемое требование.
8. При успехе durable `Committed`, затем разрешить VPN reconnect. Previous сохраняется минимум до следующего стабильного запуска/политики retention. Раннее удаление backup сразу после первого окна UI не нужно.
9. При ошибке остановить только candidate-owned processes, восстановить pointer и SCM по journal, запустить previous; записать `RolledBack`. При временном AV lock не уничтожать обе версии: показать recovery состояние и повторить локально.
10. После reboot launcher/updater восстанавливает незавершённые `SwitchIntent/Activated/HealthPending` до обычного старта. Безопасный default — последняя подтверждённая версия. Durable commit должен переживать process crash; поведение power loss проверяется на VM. Абсолютную защиту от физической порчи диска/удаления администратором обещать невозможно.

Схема настроек также участвует: новая версия не должна необратимо мигрировать единственную копию user DB до commit. Нужны обратносуместимые изменения или отдельная проверенная копия/снимок плюс схема возврата. Credentials остаются в существующем защищённом storage; их не копировать в update logs.

Сетевой supervisor после handoff не обязан скачивать что-либо. При опциональной фоновой загрузке в нём: explicit proxy policy, no_proxy только для исключения HTTP proxy, отдельные connect/read/overall limits, restart after route change, jitter/backoff, Retry-After для 429/503. Работа в любой сети невозможна, но сетевой отказ обязан оставлять подтверждённую версию запускаемой.

## 10. Миграция без обрыва старого канала

1. Зафиксировать воспроизводимую сборку и сохранить generated NSIS, binaries/hashes, текущие тесты и диагностический baseline. Устранение local dependency mismatch — отдельная подготовка, не архитектурная миграция.
2. Ввести redacted transaction/result journal и явные UX стадии, machine lock, download deadlines/повторы. Это снижает риск, но **не объявлять in-place установку атомарной**.
3. Создать и протестировать supervisor/version directories/launcher с crash recovery отдельно от live release channel. Существующий maintenance использовать как библиотеку/исполняемый recovery component.
4. В переходном **подписанном NSIS**, совместимом со старым plugin, добавить новый supervisor и candidate version directory без удаления старой рабочей установки. Создать проверенную legacy baseline и durable mapping. Затем quiesce, SCM/pointer switch, health/rollback. Первая миграция — самая опасная; запрещено сначала запускать старый uninstaller.
5. Выпустить через тот же latest.json и minisign trust. Сохранять asset формат, понятный старым клиентам, пока они не мигрировали. Новый manifest можно публиковать дополнительным asset; не заставлять старый клиент читать новую схему.
6. После подтверждённого adoption новые клиенты используют supervisor staging/health. Legacy NSIS остаётся bootstrap/repair entry с теми же transaction semantics, а не второй независимой системой установки.
7. Включать required signed version только после проверки signer/trusted comments всех поддерживаемых переходов. Подготовить ротацию ключей и совместимые recovery releases заранее.
8. Ступенчатое распространение по поддерживаемым Windows/правам/старым версиям. Удалять legacy layout только после durable commit и retention, отдельно от критического switch. Выпуск/PR на этом этапе не выполняются.

## 11. UX и logging

Состояния: Проверка → Доступна X → Загрузка (bytes/total, retry) → Проверка файла → Подготовка → Завершение сетевой сессии → Установка/активация → Перезапуск → Проверка запуска → Завершено. Отдельные исходы: отменено до handoff; требуется действие; ошибка с сохранённой текущей версией; откат выполнен; требуется локальное восстановление.

Сообщение «Текущая версия не изменена» разрешено только до файловой/activation mutation. После maintenance Abort: «Обновление остановлено до замены файлов. Сетевая сессия завершена/восстановление требует внимания. Можно запустить прежнюю версию». После rollback — указать факт и версию, а не просто общий error. Quiet NSIS не должен быть единственным каналом ошибки после закрытия UI.

Журнал JSONL в ACL-защищённом machine directory, пользовательская redacted копия/просмотр после запуска. Поля: transactionId, sequence, UTC и monotonic duration, from/to version/build, manifest/release URL без токенов, asset ID/name/size/hash, подпись/key ID/version result, HTTP status/phase/redirect host/bytes/ETag/retry/nextDelay/error category, interface LUID и route generation, VPN/session state, shutdown stage/owned process handles+PIDs+exit codes, TUN baseline, updater start/SCM switch, old/new pointer generations, backup/staging integrity, health ACK/timeout, rollback reason/result. Flush на критических переходах; rotation не удаляет единственный recovery record.

Не записывать VLESS UUID, subscription URL/token, passwords, query strings CDN signed URLs, Authorization/Cookie или raw конфигурации. Не пересылать сам пользовательский VLESS-ключ в тестовые отчёты. Проверить redaction автоматическими canary-secret тестами. Логи являются evidence, журнал состояния — recovery authority; произвольный log line не должен определять, какую программу запускать elevated.

## 12. Обязательная тестовая матрица и критерии приёмки

Все destructive tests — disposable Windows VM с реальными подписанными пакетами, службой, TUN и точками контролируемого отказа. Общий invariant: после каждого отказа/reboot запускается последняя подтверждённая версия, либо явно доступно локальное recovery без интернета; её файлы не удалены. Наличие работающего UI не заменяет проверку службы и сетевого baseline.

| Сценарий | Проверяемый результат |
|---|---|
| VPN выключен / включён | Одинаковый verified package; connected case с реальными routes, WFP и DNS; новый UI+SCM согласованы |
| VPN disconnect при download; смена сервера/IP | Bounded failure/retry, сохранённый partial с проверенной identity, старая версия запускается |
| VPN выключается перед install | Final baseline сходится идемпотентно; нет гонки reconnect |
| GitHub unavailable / API 403/429 / CDN unavailable / 5xx | Отдельная классификация; Retry-After/backoff; канал старой версии не затронут. API сценарий отдельно для publish |
| DNS failure / TLS failure / timeout / redirect loop | Bounded deadlines, TLS не отключается, не выполняется неверный пакет |
| Нет интернета; интернет вернулся | Повтор загрузки безопасен; полностью staged install/rollback работают offline |
| Обрыв download на 10%, 50%, 99% | Ни один partial не исполняется; корректный resume либо restart; конечные hash/size/signature |
| Некорректные Content-Length, повреждение, wrong hash/signature/version/platform | Отклонение до quiesce; no downgrade/replay legacy обхода в новом протоколе |
| Нет места до download / staging / journal / switch | Недеструктивный отказ либо восстановление по journal; previous не удаляется |
| Нет прав / UAC cancel / иной UAC user | Предыдущая версия сохранена, restart под исходным user; чёткий статус |
| Atlas.exe/DLL/core locked; AV quarantine | Никакой частичной перезаписи active version; bounded wait; previous доступен |
| Desktop не завершился / child не завершился / access denied | Installation gate закрыт; только доказанно owned handles; чужой same-name process жив |
| Новый desktop стартует между cleanup и switch | Machine lease блокирует session/start; race воспроизводится тестовым barrier |
| Повтор update / два updater / installer вручную | Один владелец транзакции, второй получает существующий ID/status; не два file writers |
| Crash старого Atlas при download и shutdown | До mutation previous цела; maintenance/reconciler завершает свой stage |
| Crash updater на каждом durable переходе | Новый запуск reconciles идемпотентно, без удаления last-known-good |
| Reboot VM до/после записи journal, SCM switch, pointer replace, health, commit | Проверка всех окон, согласование файлов/SCM/DB, bounded восстановление |
| Новый Atlas не запускается / сразу падает / hangs / ACK чужого PID | Health reject, остановка candidate, подтверждённый rollback previous |
| Установка successful, нет публичной сети | Локальный health не требует внешнего сайта; успех установки отделён от доступности VPN-провайдера |
| Rollback сам прерван reboot/lock | Следующий recovery продолжает возврат; обе версии сохранены до commit |
| DB migration fail / candidate записал новую схему | Previous получает совместимые данные; не просто старый EXE поверх несовместимой DB |
| Потерян temp, tampering, reparse path, raw secret в error | Не исполняется подменённый файл; ACL/подпись проверены; журнал не содержит canary secrets |
| Обновление 2.2.3, 2.3.1, текущего кандидата; offline repair; uninstall | Проверена вся поддерживаемая migration chain, различены binary rollback и settings backup |

Существующие тесты доказывают части цепочки: подпись/пакет, независимость maintenance от CEF, exact-path termination, освобождение настоящего Wintun без production routing, UI startup на CI. Они **не дают** доказательства перечисленных crash/reboot/rollback гарантий. На этапе аудита новая fault matrix не запускалась и updater не переписывался.

Итоговый приоритет: сначала сохранить рабочую версию и обеспечить транзакционный recovery (U01–U03/U10), одновременно сделать наблюдаемым handoff; затем устойчивость загрузки и полная VM-матрица. Дополнительные try/catch поверх текущей перезаписи файлов принципиальную проблему не устранят.
