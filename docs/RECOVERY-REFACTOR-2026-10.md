# Atlas 3.0.1: диагностика и восстановление, 14 октября 2026

PR подготовлен поверх актуального `main` (`972af18`, версия 2.5.1).
Первоначальный аудит и сравнение планировщиков выполнены 13 октября на базе
`4a6ed36`; результаты той сборки ниже сохранены как исторические свидетельства.
Итоговая версия 3.0.1 сохраняет изменения запуска, source-scoped node IDs,
транзакционного updater и подписок из 2.5.1. Финальные проверки PR указаны в D/G.

## A. Аудит и подтверждённые причины

Стек: React 19 / TypeScript / Vite, Tauri 3 alpha с CEF, Rust,
Windows Network Service, Mihomo и отдельные управляемые Xray-процессы.
Запуск: `src-tauri/src/main.rs` → `lib.rs::run`; UI — `src/main.tsx`.
Данные — SQLite (`storage.rs`), URL подписок — Windows Credential Manager.
Провайдерские форматы разбирают `subscriptions.rs`, `subscription_options.rs`,
`xray_config.rs`. Xray lifecycle находится в `xray_runtime.rs`; TUN/WFP —
`broker.rs`, `core.rs`, `network_guard.rs`. Эти механизмы не заменялись.

| Подтверждённый дефект в исходной версии | Место и доказательство | Исправление / риск |
|---|---|---|
| Монитор запускается раз в 10 секунд | `Recovery::monitor`, `Duration::from_secs(10)` | Период 1 с, один незавершённый монитор; больше контрольного трафика |
| Поиск ждёт три медленных результата, затем два подтверждения и hold 60 с | `poll_scheduled`, `slow_samples < 3`, `count < 2` | Первое превышение запускает единый поиск; cooldown применяется к последующим переключениям |
| Резерв не обязан укладываться в пользовательский порог | `verify_replacement`, `passed=result.is_ok()` | Проверяется `delay <= auto_search_ping_ms` |
| Успех определяет только GET селектора | `Recovery::tick`, `confirmedSelection` | После PUT отдельная проверка узла, HTTP 204 через работающий proxy listener и повторное чтение селектора |
| Поиск обслуживают только два рабочих потока с URL timeout 10 с | `verify_replacement`, `min(2)`, `verified_probe` | Шесть кандидатов, индивидуальный timeout 1500 мс; общий существующий gate — восемь сетевых запросов |
| Ручной batch повторяет timeout-записи с 10-секундным ожиданием и скрывает ранний timeout | `discover_batch`, `DISPLAY_TIMEOUT_MS` | Обычный массовый запуск использует один короткий проход; отдельная проверка узла сохраняет длинный timeout |
| Одиночная ошибка активного узла инициирует download | `subscription_refresh::failed_nodes`, `active_node_sample` | Восстановительные downloads запрашиваются только координатором после исчерпания кандидатов |
| При перестановке провайдером узлы меняют имена | `install_subscription`, suffix `i + 1` | Имена неизменившихся конфигураций сохраняются по fingerprint без display name |
| Устаревшие задержки продолжают выглядеть свежими и участвовать в сортировке | `latencyLabel`, сортировка по `delay` | Явный STALE через 30 с, исключение из сортировки свежих результатов |
| Для native option нет общей непрозрачной поверхности | `frosted.css`, только общий `select` | Общие theme tokens для option/optgroup, выбранного и disabled состояния |

Постоянного жёлтого оформления «Обновления» в текущем исходном коде не было:
кнопка уже создавалась общим `nav.map`. Это не объявляется найденным дефектом.
Проверено обычное фиолетовое active-состояние; жёлтый задан только
`.version .sidebar-update`, которую React показывает при `availableUpdate`.
Скриншотов исходного дефекта во вложении не было.

Исходная цепочка: сетевой наблюдатель / service health → `Recovery::tick` →
worker probe → канал результатов → команда селектора в service loop →
опрос `/proxies` интерфейсом. Фоновые задачи используют конечные blocking
HTTP-запросы вне React, bounded service query queues, generation cancellation,
каналы Rust и mutex защищённых снимков. UI polling не управляет переключением.
Сервис проверяет процесс/guard независимо от активного HTTP-монитора.
Существующие журналы — core events и `incident_history`; native уведомление о
таком отказе добавлено отдельно в desktop process, не в Session 0 службы.

Основные таймеры и границы фоновой работы после изменений:

| Контур | Период / граница | Защита от наложения |
|---|---|---|
| Recovery service tick | 100 мс | Единственный command loop |
| Active node + live listener | Старт раз в 1 с; конечные HTTP deadlines | Один receiver active |
| Process / TUN guard health | 1 с | Один background health probe |
| Uplink observer | 2 с, debounce 2 с | Один pending network sample |
| Native delay requests | Общий gate 8, отдельные service query jobs | bounded admission и generation cancellation |
| Subscription scheduler | 500 мс; 200 мс во время refresh | Два download; lease на subscription ID |
| Retry / refresh | 10–300 с / не чаще 300 с по умолчанию | operation ID + уникальный refresh ticket |
| UI snapshot observer | 2 с | Revision check, try_lock |
| ActiveServer / passive pool UI | 5 с | pending flag; только чтение |
| Pool history freshness | Не более 30 с | Не зависит от интервала полного сканирования |
| Atlas updater | Запуск и каждые 6 часов | Существующий updatePolling |
| Incident snapshot | 15 с | Существующий coalescing capture |

Браузерный pool observer использует те же контрольные URL и предел
свежести 30 с; прежнее окно до 10 минут больше не выдаёт старую историю за
свежий результат при большом `autoTestIntervalSeconds`.

## B. Существенные изменения файлов

- `resilient_selection.rs`: единый путь автоматического выбора, фазы,
  operation ID, threshold, карантин, bounded scan, подтверждение, rollback,
  finite attempted set, backoff и одноразовые refresh tickets.
- `latency.rs`: короткий массовый проход, дедупликация одинаковых конфигураций,
  QUEUED/CHECKING progress, время фактического результата, раздельные timeout /
  unreachable, структурированные probe events, воспроизводимый load profile.
- `core.rs`: HTTP deadline учитывает requested timeout; отдельный запрос
  через live listener; ограниченные recovery IPC-команды.
- `broker.rs`: tick координатора 100 мс, process health 1 с, recovery status,
  completion ticket; сохранение incident при атомарном subscription apply.
- `subscription_refresh.rs`, `lib.rs`: два параллельных download, единый commit
  актуального settings snapshot, защита revision/shutdown, stable names,
  публикация recovery state, уведомление один раз на incident.
- `model.rs`, `config.rs`: обратно совместимая `recoveryPolicy` с defaults и
  валидацией; изменение политики доставляется службе без пересоздания TUN.
- `recovery_notification.rs`: нативный Windows Shell notification из desktop,
  отдельные тексты успешного и неуспешного обновления подписок.
- `latency.ts`, `ActiveServer.tsx`, `ServerCard.tsx`, `main.tsx`, `types.ts`, `usePoolRecovery.ts`:
  свежесть, последняя успешная попытка, метрика, сортировка, recovery indication.
- `frosted.css`: общая тема native options и специальной кнопки обновления.
- `recovery_tests.rs`, `latency.test.ts`, `scripts/test-recovery-load.ps1`:
  локальные отказные сценарии и измерение scheduler workload.
- `test-ui-startup.ps1`: проверка завершения своих процессов через Get-Process;
  WMI/CIM оказался недоступен в sandbox, рендер и IPC до этого уже прошли.

До начала работы уже были изменены `core.rs`, `network_guard.rs` и существовала
незакоммиченная папка `docs/audits/`. Эти изменения сохранены. Существующий diff
`network_guard.rs` не является результатом данного рефакторинга.

## C. Фактическая логика

1. Служба планирует активную проверку с периодом 1 с. Если предыдущая не
   закончена, новый worker не запускается. Это период запуска, не обещание
   завершения сетевого запроса за секунду. Минимизация окна не влияет на службу.
2. Метрика порога — HTTP latency через конкретный узел, миллисекунды.
   Deadline — `max(1500, autoSearchPingMs + 250)` мс (не более 10000),
   чтобы разрешённый пользователем большой RTT не превращался в искусственный
   таймаут. Listener verification использует тот же бюджет.
   Она не называется ICMP/TCP RTT и не заменяет проверку передачи данных.
   Проверяются ожидаемый HTTP status, listener traffic и состояние процесса.
3. В AUTO/FAILOVER первый отказ создаёт incident. При включённой
   `autoOptimize` первое превышение порога также немедленно начинает поиск.
   Отключённая настройка сохраняет медленный работающий сервер, но не блокирует
   восстановление при подтверждённом отказе. Ручное закрепление
   узла сохраняет прежнюю семантику: Atlas диагностирует его, но не выбирает
   самовольно другой. Direct mode не включает автоматическую смену VPN-узлов.
4. Список кандидатов исключает quarantine и уже неудачно проверенные в данном
   цикле переключения. Одинаковые конфигурации с разными названиями проверяются
   один раз; разные credentials/SNI/transport не объединяются по IP:port.
5. Шесть workers предлагают кандидата, первый свежий результат ниже порога
   даёт fast path. Неполный scan / локальная ошибка контроллера приводят к retry,
   а не к ложному выводу, что подписки не работают. Общий scan deadline:
   `max(4 seconds, ceil(candidate_count / 6) * 2 * (probe_timeout + 500 ms))`.
   Удалённый отказ HTTP подтверждается независимым HTTPS-контролем Google;
   локальная ошибка не вызывает второй запрос. При проверке listener также
   допускается успешный HTTPS-контроль.
6. Только service command loop изменяет селектор. Процесс/TUN не пересоздаются
   при смене узла. До выбранного кандидата работающий путь сохраняется.
   После выбора выполняются новый node check, HTTP 204 через live listener,
   проверка выбранного имени. До их завершения фаза VERIFYING.
7. При неудаче возвращается прежний селектор; это само по себе не считается
   подтверждением здоровья прежнего пути. Кандидат карантинируется и не может
   бесконечно переизбираться в том же проходе. Ручная команда инвалидирует
   generation, receivers и refresh ticket; поздний результат не коммитится.
8. После полного исчерпания — REFRESHING_SUBSCRIPTIONS. Desktop скачивает
   только URL-источники; вставленные VLESS-ключи не считаются URL-подписками.
   Имена восстанавливает общий `subscriptions::reconcile_nodes`, ссылки
   избранного и выбранного узла — `repository::reconcile_references`. Скачивает
   подписки парами, проверяет snapshot revision, валидирует конфигурацию и
   сохраняет успешные обновления одной транзакцией через существующий save /
   apply / rollback. Неудачные источники сохраняют прежние узлы.
   Между парами проверяются shutdown и общий бюджет загрузок 120 с.
9. Новый уникальный ticket отличает каждый разрешённый refresh одного incident.
   Устаревшее completion не может завершить новый refresh. После завершения —
   RESCANNING. После неудачи — RECOVERY_FAILED → WAITING_RETRY и уведомление.
10. Defaults `recoveryPolicy`: retry 10, 20, 40, 80, 160, 300 с, далее 300 с;
    повторное обновление не чаще 300 с; latency switch cooldown 30 с. Полный
    отказ имеет приоритет над cooldown. Политика хранится в settings JSON с
    serde defaults; отдельного нового экрана не добавлено.

События содержат timestamp, operation/configuration version; каждый probe —
собственный probe ID и monotonic elapsed time. Пароли и URL подписок в новые
recovery events не записываются. UI видит не только «процесс работает», но и
неподтверждённый/восстанавливаемый путь. React сохраняет ключи строк и
применяет прогресс отдельных узлов; таблица не пересоздаётся целиком.
Сохранён справедливый фоновый обход резервов из 2.5.1: по два измерения,
ограниченная очередь и один незавершённый job. Он обновляет samples, но не
изменяет селектор: все переключения выполняет новый координатор.

## D. Выполненные проверки

Для PR 3.0.1 повторно прошли 54 frontend tests и 51 script contract tests.
Первый sandbox run выявил недоступность Windows Credential Manager, TLS
certificate и временного HKCU-раздела в унаследованных тестах. Эти проверки
успешно повторены вне sandbox; проверки пользовательской рабочей сети не выполнялись.
Итоговый Rust run для PR: **378 passed, 0 failed, 6 ignored** — library 260,
AtlasMaintenance 50, AtlasUpdater 65, main 3. Ignored остаются нагрузочный
profile, тест с личным VLESS и отдельные административные/child fixtures;
их наличие не скрывается счётчиком успешных тестов.
Команда: `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
--bin atlas-vpn --bin AtlasMaintenance --bin AtlasUpdater`.
Сценарий отключённой автооптимизации отдельно подтверждает: медленный живой
путь сохраняется, отказ всё равно запускает recovery.

Baseline: 37 frontend tests; 180 Rust tests, все успешны.

После изменений: 39 frontend tests; финальный полный Rust run — 188 успешных,
0 failed, один нагрузочный test помечен ignored и выполнен отдельно.
Продолжительность финального Rust run — 44,54 с. В него включены повторный
refresh ticket и проверка бюджета timeout для большого пользовательского порога.

Новые сценарии используют loopback HTTP controller/proxy, без рабочих
подписок и без изменений маршрутов Windows:

- 80/150 мс: нет поиска; 200/150 мс: поиск; TIMEOUT и приоритет полного отказа.
- Кандидат 60 мс; отдельное VERIFYING перед успехом.
- Кандидат выше порога не выбирается.
- Неисправный listener: нет RecoverySucceeded, возврат предыдущего селектора.
- Отсутствие кандидатов: один refresh; rescan; failed/retry без повторного
  incident от следующих timeout.
- Успех после refresh; stale generation и поздний refresh ticket отвергаются.
- Подписка переставляет конфигурации: прежние имена сохраняются.
- Старая задержка не сортируется как свежая; timeout, error, queue различимы.

Существующий общий suite включает реальные изолированные Mihomo/Xray,
VLESS, потерю и восстановление серверного процесса, группы
проверок из 62 узлов, DNS/rules, конфигурационный rollback, освобождение
процессов/слушателей и 50 циклов probe-only core. Эти тесты прошли.

Границы обязательной регрессии:

| Возможности | Полученное подтверждение / оставшаяся проверка |
|---|---|
| Подключение, отключение, ручной выбор | Изолированные core lifecycle, selector/reload/rollback и cancellation tests; клики в установленном VPN с реальным TUN не проверены |
| AUTO, резервное переключение | Loopback recovery scenarios и реальный VLESS fail/recovery в общем suite |
| Подписки, VLESS-ключи, URL | Parser/metadata/refresh/stale revision/stable names tests; загрузка личных провайдеров не выполнялась |
| Избранное, поиск, сортировка | Сохранение имён и storage round-trip; тесты свежести/ранжирования; полный UI-сценарий избранного и поиска не выполнен |
| Правила и DNS | Unit tests и реальные изолированные Mihomo/Xray DNS/route tests |
| Диагностика, журнал, настройки | Diagnostic evidence, redaction, incident history, storage, legacy defaults и config validation tests |
| Проверка обновлений | Frontend updatePolling tests и verifier подписи/изменения байтов; установка обновления не выполнялась |
| Уведомления Windows | Компиляция и тест текста при неудачном refresh; видимый системный popup не проверен |
| Автозапуск, свёрнутое окно, повторный запуск | Служебная логика не зависит от React; изолированный запуск/выход проверен smoke-скриптом; автозапуск и длительная работа свёрнутого установленного VPN не проверены |

Эта матрица не утверждает полную ручную регрессию установленных Windows-функций.

Браузерная проверка собранного UI: «Обновления» открывает нужный раздел,
стиль соответствует nav; кнопки «Обновить» нет без availableUpdate.
На изолированном стенде с фактическими CSS: option foreground
`rgb(209,228,250)`, opaque background `rgb(11,14,25)`, disabled foreground
`rgb(157,167,186)`; ArrowDown/Enter выбирают VLESS. Проверены CSS zoom
100%, 125%, 150%, 200%. Это **не native Windows DPI acceptance**.

## E. Производительность

`RECOVERY-LOAD-2026-10.json` содержит все p50/p95/p99/max и методику.
Это сравнение **планировщиков на масштабированной модели ожиданий**, не
измерение Интернета: 20% отказов; задержки 1/3/20 мс вместо реальных RTT и
1500/10000 мс. По 10 повторений каждого размера и каждой политики.

| Узлов | Полный проход до, p50 мс | После, p50 мс | После p95 / p99 / max, мс |
|---:|---:|---:|---:|
| 10 | 141.4 | 100.6 | 100.8 |
| 50 | 201.2 | 100.6 | 101.0 |
| 100 | 222.6 | 100.6 | 100.9 |
| 500 | 610.4 | 206.0 | 206.9 |
| 1000 | 1091.8 | 357.6 | 364.8 |

При 10 повторениях nearest-rank p95/p99 совпадают с max; такая малая выборка
не позволяет утверждать tail latency реальной VPN-сети. Первое успешное
завершение — примерно 5 мс в этой модели. Предел — шесть workers.
Весь измерительный процесс: 31.657 с wall time, 1093.75 мс CPU,
peak working set 11 730 944 байт. Это не память всего Atlas с CEF/Mihomo/Xray.
Причина минимального времени около 100 мс — сохранённый stagger рабочих
потоков 20 мс. Увеличивать общий gate до 16 без сетевых измерений не стали.

Реальные гарантии ограничены: HTTP deadline составляет requested timeout
плюс 500 мс controller grace; существующий admission gate имеет собственный
предел ожидания. Общий scan deadline прекращает ожидание/выдачу новых задач;
уже начавшиеся requests завершаются по своим конечным deadlines. Задачи
не создаются по одной на каждую запись списка. Численные цели 200 мс / 2 с /
4 с не объявляются гарантированными для любой сети.

## F. Совместимость

SQL schema, keyring service name, протоколы, форматы подписок, правила, DNS,
WFP ownership и updater trust не изменены. Старые settings читаются с
default recoveryPolicy, что проверено тестом. Списки favorites и rules
не очищаются; имена идентичных серверов сохраняются при reorder. При полном
изменении провайдером параметров узла он остаётся новой конфигурацией: старый
favorite не удаляется, но соответствие совершенно новому узлу не выдумывается.
Существующий apply сохраняет validation/rollback. Рабочий профиль пользователя
не использовался для отказных тестов и не редактировался.

## G. Сборка

Ниже сначала зафиксирована первоначальная сборка 2.3.1-alpha.1 от 13 октября.
Она не является артефактом PR 3.0.1. Для нового PR применяется существующий
`build-release-installer.ps1`: полный transactional installer с receipt-bound
AtlasUpdater; промежуточный Tauri setup не предназначен для распространения.

Команды из корня PowerShell:

```powershell
. ./scripts/initialize-msvc.ps1
$env:CARGO_TARGET_DIR = Join-Path (Get-Location) 't'
npm.cmd test
npm.cmd run build
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo test --manifest-path src-tauri/Cargo.toml --lib scheduler_load_profile -- --ignored --nocapture
./scripts/test-recovery-load.ps1 -Executable t/debug/deps/atlas-0b9fa84c02ef3b93.exe
```

Release собран Tauri CLI с CEF_PATH на уже имеющийся workspace CEF.
Локальный override отключает только создание updater-signature artifacts
и задаёт NSIS zlib; проверенный updater public key в исходниках не изменён.
Финальная команда: `npm.cmd run tauri -- build --bundles nsis --config
temp/atlas-build-config.json`. Это локальная unsigned сборка, не публикация.
Финальный build завершился с exit code 0; release-компиляция заняла 2 мин 33 с.
Получены файлы (пути относительно корня репозитория):

| Артефакт | Размер, байт | SHA-256 |
|---|---:|---|
| `t/release/Atlas.exe` | 157368832 | `D7A73B4053CC85A0AC310F6EC486620DE67938E6813AF4E5C94FCFDDDE0BAC9E` |
| `t/release/bundle/nsis/Atlas_2.3.1-alpha.1_x64-setup.exe` | 265507590 | `340AF320CC164117FAA7DF24C5EAF5B1578101D8A8EB3E43C9DCBD9194AC60FD` |

NSIS сообщает 253,21 MiB. Оставшиеся предупреждения: большой Vite chunk,
неиспользуемая `subscriptions::parse`, информационное сообщение linker и
canonicalize родительского пути. Они не остановили production build.
Проверка `node scripts/verify-installer.check.cjs` прошла: verifier отклоняет
изменённые bytes/key/trusted comment. Это тест verifier, не утверждение о
наличии цифровой подписи у данного локального пакета.

На финальном `Atlas.exe` изолированный `test-ui-startup.ps1` подтвердил
версию `2.3.1-alpha.1`, `rendered=true`, 12 кнопок,
работающий IPC и отсутствие оставшихся процессов. `test-service-identity.ps1`
подтвердил, что Atlas.Service.exe принимает своего Atlas.exe-владельца.
Оба smoke mode обходят настройки, SCM и запуск VPN. Первоначальный вызов UI
скрипта остановился на недоступном WMI после успешного рендера; проверка
завершения переведена на разрешённый Get-Process и повторно прошла целиком.
Логи: `temp/atlas-release-final.log`, `temp/atlas-tests-rust-final.log`,
`temp/atlas-tests-ui-final.log`, `temp/atlas-ui-smoke-final.log`,
`temp/atlas-service-identity-final.log`. `git diff --check` прошёл.

## H. Ограничения и оставшаяся приёмка

- Реальное прохождение Windows TUN в рабочей сети, service crash под
  ограниченным пользователем, смена Wi-Fi/DNS, installed upgrade/uninstall и
  многочасовой soak не запускались: они затрагивают рабочую сеть/установку.
- Listener HTTP + отдельный node probe + существующая проверка TUN/WFP дают
  разные свидетельства. HTTP через explicit proxy не является доказательством
  прохождения контрольного запроса через Windows TUN. Нельзя выдавать его за
  такую end-to-end проверку; требуется отдельная приёмка на изолированной VM.
- Windows notification API добавлен и компилируется; показ реального balloon,
  Focus Assist и native DPI 100/125/150/200% здесь не подтверждены.
- Нет замеров реальных p50/p95/p99 Detection/Search/Switch/Verification/Total
  на удалённых VPN при контролируемой потере пакетов. Есть monotonic events,
  функциональный loopback стенд и отдельная нагрузочная модель.
- Существующий config apply/SCM lifecycle остаётся синхронной service-командой;
  изменение подписок может ждать network validation. UI-поток не выполняет
  эти операции, но полную отзывчивость IPC во время reconfigure эта работа
  не доказывает. Фоновый monitor не запускает накладывающиеся тики.
- Обновление бинарников ядра, подписывание и публикация релиза не выполнялись.
  Сборка и тесты не заменяют полную регрессионную приёмку установленного VPN.
