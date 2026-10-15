HISTORICAL EVIDENCE — source workspace release/2.4.2.
These results describe the earlier stage, not validation of main/v3.0.3.
For this integration see docs/HEALTH_WINDOWS_INTEGRATION.md. Local temp logs are not committed.

# ATLAS — Health-check integration and Windows lifecycle

Дата: 15 октября 2026, Europe/Moscow. Изменения подготовлены в рабочем дереве; установка и выпуск не выполнялись.

## 1. Executive Summary

Исправлены три подтверждённых дефекта исходного кода: потеря повторной активации до Tauri single-instance callback, привязка Shell к версионированному desktop EXE, стирание timeout при начале следующей проверки.

Реальный изолированный Windows/CEF тест восстановил одно существующее окно после закрытия, скрытия и минимизации, через временный `.lnk` и при восьми быстрых повторных запусках. Фокус получен. Установленный Atlas не использовался как тестовый экземпляр.

Настоящий Mihomo выполнил HTTPS HEAD через конкретный SOCKS outbound с проверкой TLS и HTTP 204. Зависшие handshake/traffic дали HTTP 504, `delay=null`; восстановление дало новый успех. Компоненты карточки и активного сервера проверены настоящим React render.

Windows pin → upgrade → повторный запуск в отдельной VM не выполнен. Поэтому исправление причины taskbar pin реализовано, но полная совместимость обновления и готовность к выпуску пока не подтверждены. Старые пользовательские pins не переписывались.

## 2. Current State

- Branch: `release/2.4.2`; HEAD: `eca2f9ed7ceeedf9e2ac70d6197340a20b819ec5`.
- Версия package/Cargo/Tauri config: 2.4.2. Установленная копия: 3.0.3, build `a243a40bdaeb276ef3718af60c1de60bb56db466`. Это разные состояния; проверка workspace не является проверкой установленного релиза.
- Фактический runtime: Tauri 3.0.0-alpha.2 / CEF 3.0.0-alpha.2; single-instance 3.0.0-alpha.1. Схема JSON `/config/2` не означает runtime Tauri 2.
- Windows x64; NSIS per-machine, `Program Files\Atlas`, HKLM uninstall identity `Atlas`, product/main binary `Atlas` / `Atlas.exe`, bundle ID `net.atlasvpn.desktop`.
- Release script доставляет receipt-bound transactional NSIS wrapper. Стандартный Tauri NSIS служит payload. Существующий native updater выбирает версию по `current.json`; root `Atlas.exe` является стабильным launcher, desktop находится в `versions/<version-build>/Atlas.exe`.
- Tauri updater endpoints, trust key, quiet install mode, транзакционный rollback и служба сохранены. MSI/WiX не используются в этом пути.
- Прочитан `docs/server-health-audit.txt`. Его fixes уже находились в dirty tree. Изменения `update_user.rs` (47 добавленных строк), DIRECT/TSPU и другие прежние материалы сохранены. Нового коммита, смены версии и release artifacts нет.

## 3. Root Cause — Desktop Activation

В локально закреплённой зависимости `tauri-runtime-cef-3.0.0-alpha.2/src/cef_impl/client/process.rs` обработчик `on_already_running_app_relaunch` обрабатывает зарегистрированные deep links, а обычный запуск заканчивается `TODO: add event` и `return 1`. Chromium захватывает повторный запуск по общему cache раньше plugin setup. Поэтому существующий callback Atlas с `show/set_focus` часто вообще не вызывается.

Второй дефект: ни callback, ни tray handler не вызывали `unminimize`. Третий: запрос до создания main window терялся. Сам `DesktopJob` не был single-instance mutex: он создаёт UUID на каждый desktop и контролирует только жизнь CEF helpers.

Штатный Windows plugin использует endpoint `<identifier>-sic/-siw`, `WM_COPYDATA` с tag 1542 и передачу foreground permission. Эти имена/формат проверены в закреплённом исходнике plugin. Его окно остаётся единственным IPC endpoint; новый native код отправляет только фиксированную команду активации до запуска CEF.

Добавлен startup mutex-barrier до runtime и до application setup, удерживаемый до `RunEvent::Ready`. Конкурирующий первый запуск ждёт kernel mutex, затем повторно ищет существующий endpoint. Ошибка доставки/таймаут прекращает второй запуск до сетевого cleanup, а не разрешает второй VPN. Запрос до Ready хранится как pending и выполняется после startup hide. Нет production polling или искусственных задержек.

Существующая логика limited-token launcher сохранена. UIPI/недоступный endpoint не обходятся. Не выполнены UI-тесты elevated/unelevated mismatch, другого Windows session или virtual desktop; foreground остаётся под правилами Windows.

## 4. Root Cause — Taskbar Pin

Read-only исследование установленных `.lnk` сохранено в `docs/health-windows-evidence/installed-shortcuts-readonly.json`:

| Ярлык | Фактическая цель | Explicit AppID |
|---|---|---|
| Public Desktop | `C:\Program Files\Atlas\Atlas.exe` | отсутствует |
| Common Start Menu | `C:\Program Files\Atlas\Atlas.exe` | `net.atlasvpn.desktop` |
| User taskbar pin | `C:\Program Files\Atlas\versions\3.0.3-a243a40bdaeb276ef3718af60c1de60bb56db466\Atlas.exe` | отсутствует |

В исходном desktop не было явных Shell relaunch properties. Поэтому pin работающего окна мог сохранять физический EXE версии, обходя root launcher. Даже при сохранении старого каталога admission fence отклоняет неактивную версию после commit; удаление каталога также сделало бы такой pin нерабочим. Удаление именно этого каталога не воспроизводилось и не объявляется доказанной причиной конкретного прошлого обновления.

Локальные v3.0.1/v3.0.2 и текущий config используют прежний bundle ID. Уже существующий Start Menu shortcut подтверждает `net.atlasvpn.desktop`; новый произвольный ID не вводился. Версионированный payload сохранён для updater, стабильной целью Shell становится существующий root launcher.

## 5. Root Cause — Timeout

В `probe_state::State::begin` запись безусловно заменялась на `testing`, теряя предыдущие `error/errorKind`. Frontend `test` и `testAll` делали такой же reset. Открытие Servers запускает свежую пачку, поэтому наблюдавшийся timeout исчезал ещё до появления нового результата.

Дополнительно HTTP 503 и HTTP 504 оба превращались в `unreachable`, а UI показывал этому статусу «Таймаут». Это ошибочно называло обычный transport failure таймаутом. Polling failure мог также стирать прежний подтверждённый отказ.

Теперь pending — внутренний флаг текущей работы. Последний завершённый отказ сохраняет причину и время измерения до нового результата; operation/start ordering продвигается для защиты от старого success. Начало проверки положительного результата по-прежнему убирает старое зелёное утверждение. Успешное завершение очищает прежние error fields, завершение любого запроса снимает pending. Config/network invalidation очищает evidence, а не сохраняет старый отказ навсегда.

HTTP 504 = `unreachable/timeout`; HTTP 503 = существующий `error/proxy_or_transport`. Ошибка controller/deadline IPC не является доказательством proxy timeout. UI-структура и названия существующих статусов не изменены.

## 6. Implemented Changes

1. Единый `window_activation::request`: очередь до Ready, coalescing, main-thread `show → unminimize → set_focus`; тот же путь для tray и second-instance. CloseRequested продолжает скрывать окно.
2. Pre-CEF forwarding в существующий Tauri endpoint, bounded send acknowledgement и startup barrier. На пути повторной активации нет App/database/service/VPN setup.
3. Явные process/window AppID и window `RelaunchCommand`, `RelaunchDisplayNameResource`, `RelaunchIconResource`; команда указывает на root `Atlas.exe`, иконка тоже стабильная.
4. NSIS после создания собственных Desktop/Start Menu shortcuts вызывает native `--stamp-shortcut`. Он проверяет target, задаёт прежний AppID, сохраняет shortcut; чужая цель отвергается без изменения файла. Pins не читаются и не меняются этим production кодом.
5. Backend и frontend сохраняют failure evidence при повторной проверке, отделяют pending, timeout и transport error; оба UI-компонента получают общую запись.
6. Добавлены настоящие TLS, COM/shortcut, CEF lifecycle и render regressions. Debug fixture полностью обходит production setup; release binary отказывается выполнять fixture вместо перехода к обычному запуску.

Shell implementation следует документированным [RelaunchCommand](https://learn.microsoft.com/en-us/windows/win32/properties/props-system-appusermodel-relaunchcommand) и [RelaunchDisplayNameResource](https://learn.microsoft.com/en-us/windows/win32/properties/props-system-appusermodel-relaunchdisplaynameresource). Свойства заданы через `SHGetPropertyStoreForWindow`; их значения прочитаны обратно в реальном окне. Это проверка метаданных, не доказательство upgrade pin.

## 7. Changed Files

Изменения именно этого этапа поверх прежнего dirty tree:

| Файлы | Назначение |
|---|---|
| `src-tauri/src/window_activation.rs` | Pre-CEF forwarding, barrier, единое восстановление |
| `src-tauri/src/shell_identity.rs` | Shell identity/relaunch, stamping собственных shortcuts, COM tests |
| `src-tauri/src/lib.rs`, `main.rs` | Подключение lifecycle/identity, безопасный debug fixture entry |
| `src-tauri/src/updater_main.rs` | Native shortcut stamping command |
| `scripts/transactional-installer.nsi` | AppID Desktop/Start Menu после создания ярлыков |
| `src-tauri/Cargo.toml`, `Cargo.lock` | Прямая зависимость на уже закреплённый windows 0.62.2 и нужные COM API |
| `src-tauri/src/probe_state.rs`, `latency.rs` | Pending/failure semantics, HTTP 503/504 distinction, regressions |
| `src/latency.ts`, `main.tsx`, `ServerHealth.test.tsx` | Shared pending/evidence, сохранение timeout, настоящий render |
| `src-tauri/src/lifecycle_smoke.rs`, `scripts/test-windows-lifecycle.ps1` | Изолированная Windows acceptance |
| `src-tauri/src/probe_integration_tests.rs`, `scripts/atlas-health-fixture-cert.ps1`, `atlas-health-https-fixture.mjs` | Локальный TLS origin и реальные HTTPS timeout/recovery |
| Этот отчёт, `docs/health-windows-evidence/` | Проверяемые результаты и safety baseline |

Остальные уже изменённые health-файлы сохранены и покрыты регрессией. Сравнение только с HEAD включает также работу прежних этапов, поэтому не является списком изменений этого запроса.

## 8. Windows Validation

`scripts/test-windows-lifecycle.ps1` запускает workspace debug binary с UUID-specific IPC/cache, `about:blank` вместо пользовательского приложения и собственным временным `.lnk`. Установленная копия не активируется. Evidence: `docs/health-windows-evidence/lifecycle.json`.

| Test | Фактический результат |
|---|---|
| 1 — окно открыто, второй запуск | PASS: реальное CEF-окно, один экземпляр fixture |
| 2 — скрыто, Desktop | PASS в изоляции: настоящий ShellExecute временного `.lnk` восстановил то же окно |
| 3 — минимизация | PASS: hidden + minimized восстановлены |
| 4 — tray | PASS общего production restore handler; физический клик по меню tray не автоматизировался |
| 5 — быстрые запуски | PASS: восемь вторичных процессов успешно завершились, одно main window |
| 6 — активный VPN, второй UI | Частично: установленный VPN сохранён; сценарий активации именно production UI не запускался |
| 7 — pin предыдущей версии → upgrade VM | НЕ ВЫПОЛНЕН |
| 8 — Desktop/Start/taskbar после upgrade | НЕ ВЫПОЛНЕН end-to-end; отдельные HWND/.lnk primitives проверены |
| 9 — стабильность identity/path | PASS source/COM tests и readback HWND; не заменяет upgrade test |
| 10 — hidden/minimized после upgrade | НЕ ВЫПОЛНЕН upgrade; isolated restore PASS |
| 11 — reinstall/следующий update | COM stamping дважды PASS; installer reinstall/next update НЕ ВЫПОЛНЕН |
| 12 — отсутствие вмешательства в VPN | Отдельный baseline comparison; fixture вообще не создаёт App/service/VPN |

Также PASS: pending activation до Ready переживает startup hide; focus=true в fixture. HypervisorPresent=true у физического MS-7E02 не является доступной disposable Windows VM. Готовая VM не была предоставлена/обнаружена через доступный Hyper-V cmdlet; установка на host вместо VM не выполнялась.

## 9. Health Validation

| Test | Evidence |
|---|---|
| 13 — HTTPS 204 через proxy | PASS: real Mihomo → SOCKS → TLS 1.3 HEAD → 204; сначала незнакомый сертификат отвергнут |
| 14 — handshake hung | PASS: реальный HTTP 504, timeout, delay=null |
| 15 — traffic hung | PASS: реальный HTTP 504 после SOCKS handshake; backend/UI persistence tests |
| 16 — 56 мс → timeout | PASS Rust ordering + React card/ActiveServer render; старого зелёного ping нет |
| 17 — recovery | PASS реальной HTTPS-последовательности success → timeout → timeout → success |
| 18 — поздние ответы | PASS generation/operation ordering, поздние success и failure отвергаются |
| 19 — config invalidate | PASS State.configure/invalidate и node-identity frontend tests |
| 20 — DIRECT | PASS существующего настоящего Xray full-profile/DIRECT negative test и route validation |
| 21 — STUN | PASS реального UDP sink + работающий TCP; separate evidence/render |
| 22 — Servers open | PASS scheduler immediate refresh/non-overlap/TTL + render сохранения timeout при новом begin |
| 23 — subscription refresh | PASS существующих subscription/config tests и identity-retention regression |
| 24 — ActiveServer/card | PASS одинакового timeout в реальных компонентах |
| 25 — history 56 при failure | PASS: `selectedLatency` читает atlasHealth; React не показывает history success |

Сохранились HTTPS control URL, expected 204, конкретный outbound, отсутствие DIRECT-success, весь успешный HTTP request в delay, unified-delay=false, delay=null на отказе, TTL 10 секунд, config/network generation invalidation, bounded workers/admission, cancellation, общий UI state, TCP/UDP separation. Primary URL production не менялся: `https://cp.cloudflare.com/generate_204?atlas_probe=1`.

TLS test доверяет ephemeral certificate только disposable Mihomo через его `tls.custom-certifactes` (так ключ написан в pinned Mihomo). Системное certificate store не меняется; skip-cert-verify не включён. Секретный тестовый ключ удаляется. Тест проверяет HTTPS-механизм локально, не доступность Cloudflare или конкретной пользовательской подписки.

## 10. Updater Compatibility

Сохранены root `Atlas.exe`, existing AppID, install scope/location, updater protocol, journal, signed payload verification и service ownership. Versioned payload остаётся внутренним механизмом транзакции, но больше не задаёт relaunch target окна. Same-version target вычисляется одинаково; COM stamping идемпотентен. NSIS template с dummy payload успешно компилируется; он не устанавливался и не является release package.

Предыдущие pins на root launcher имеют стабильную цель. Для pins на физический старый version EXE нельзя гарантировать исправление из новой версии: старый код может не переслать активацию, путь может отсутствовать, admission может отвергать неактивную версию. Пользовательские pin-файлы/базы не изменяются; принудительного pin/unpin нет. При таком старом повреждённом pin может потребоваться однократное ручное закрепление исправленной версии. Успешная миграция старого pin не заявляется.

Действующий updater намеренно quiesces сеть при самой транзакции обновления. Эта задача отделяет повторную UI-активацию от VPN; она не превращает существующее обновление в бесшовную замену работающего ядра.

## 11. Regression Results

Основные команды (PowerShell, repo root):

```powershell
. ./scripts/initialize-msvc.ps1
cargo test --offline --locked --manifest-path src-tauri/Cargo.toml -- --test-threads=1
cargo check --offline --locked --manifest-path src-tauri/Cargo.toml --bins
cargo build --offline --locked --manifest-path src-tauri/Cargo.toml --bins
$env:TEMP=(Resolve-Path temp/health-windows-tests).Path
$env:TMP=$env:TEMP
npm.cmd test -- --exclude 't/**' --exclude 'scripts/**' --reporter=dot
npm.cmd run build
node --test scripts/*.check.cjs
node --test scripts/atlas-routing-evidence.test.mjs
./scripts/test-windows-lifecycle.ps1
```

Результаты и завершение последнего прогона записаны в `docs/health-windows-evidence/validation.json`; полные логи — `temp/health-windows-tests/`. Финальный Rust: **213 library + 47 Maintenance + 58 Updater passed, 0 failed**; 2 library и 3 Updater tests ignored. Vitest: 45 passed в 13 файлах, включая настоящий React render. TypeScript + Vite production build: PASS. Node contracts: 19 passed; отдельный Node runner: 4 passed. Native build/check и финальный Windows fixture: PASS.

Промежуточные проблемы сохранены, а не скрыты:

- Первый compile выявил недостающую windows feature `Win32_Storage_FileSystem` для IShellLinkW.GetPath; исправлено, повторный build PASS.
- Первый общий Vitest поиск захватил файл `node:test` из scripts: 45 тестов прошли, suite discovery ошибся. Runners разделены; этот файл отдельно прошёл Node runner. Архив `t/` не является рабочим тестовым деревом.
- Первый вызов нового HTTPS test с `--exact` и коротким именем запустил 0 tests; не считается проверкой. Правильный targeted invocation и полный финальный набор выполняют тест.
- 32-bit makensis внутри sandbox не стартовал (relocated DLL); compilation вне sandbox прошла. Installer не запускался.
- Остаются предупреждения о больших Vite chunks, существующем dead code и incremental hard-link fallback; они не скрывают ошибки тестов.

Ignored: supplied VLESS subscription требует внешних credentials; административные/restricted-child updater drivers требуют отдельного режима; старый optional routing harness оставлен opt-in. Новый HTTPS fixture включён в обычный suite. VM installer/service acceptance не запускалась на production host.

## 12. Production Safety

До работы снят `baseline-before.json`, после тестов — `baseline-after.json`. Сравнение находится в `safety-comparison.json`. Проверяются PID/creation times Atlas, service state/PID, adapters, route/DNS/proxy fingerprints и оставшиеся workspace processes. Некоторые protected process paths и открытые DB hashes недоступны; это не заменяется утверждением, будто файлы точно не менялись.

Снимки 14:22:41 → 14:48:27: все 11 production Atlas PID/creation times совпадают; `AtlasNetworkService` остаётся Running, PID 2312; desktop PID 29128, Core PID 12276. Adapters, 126 routes, DNS и proxy fingerprints совпадают. Оставшихся workspace Atlas процессов нет. Общее число TCP/UDP sockets менялось с обычной работой системы; утверждение о побитовом равенстве всех сокетов не делается.

Не выполнялись production launch/restart/stop, установка, смена сервера, запись в настройки/БД Atlas, команды рабочему controller, настройка TUN/DNS/routes/proxy/WFP. Не было внешних live probes через пользовательскую подписку. Локальные fixtures имеют свои random loopback ports, процессы и Job Objects. Исходники и тестовые артефакты создавались только в workspace/temp. Снимки до/после не являются непрерывным доказательством отсутствия краткого сетевого события.

## 13. Remaining Risks

Обязательный release gate: disposable Windows VM с прежним installer, настоящим user pin, обновлением, Desktop/Start/taskbar активацией, hidden/minimized сценариями, same-version reinstall и следующим обновлением. Отдельно проверить обычный UAC split account, unsplit admin, multi-session и virtual-desktop/foreground ограничения.

Native preflight зависит от формата Windows IPC закреплённого Tauri plugin 3.0.0-alpha.1. При смене plugin/runtime повторить source audit и real lifecycle test; не обновлять dependency без этого теста. Startup имеет bounded ожидание 30 секунд, доставка IPC — 5 секунд; при зависшем первом UI второй не запускает новый App.

Не подтверждены pin migration старых физических путей и полный packaged updater install. Исправления находятся в workspace 2.4.2; перенос/сборка для актуальной release line и VM acceptance обязательны до выпуска. Нельзя использовать результат локального fixture как утверждение, что установленная 3.0.3 уже исправлена.

## 14. Final Verdict

**Desktop restore:** исправлен и проверен на настоящем изолированном CEF-окне и `.lnk`; один window/session fixture, фокус получен. **Taskbar:** причина устранена в коде и Shell metadata; pin→upgrade VM остаётся непроверенным. **Timeout:** сохраняется при pending/re-render/reopen до нового evidence; HTTP 503 отделён от HTTP 504. **Ложный зелёный ping:** не возвращён, negative tests и render tests проходят. **Предыдущие health fixes:** сохранены, полный набор регрессии прошёл.

Production Atlas не обновлён и не использован для мутационных тестов. **Release readiness: не готов к выпуску до обязательной VM acceptance**, несмотря на успешно проверенные локальные исправления.
