# Разбор 17 находок аудита

Исправление кода не означает installed acceptance. Исходный аудит хранится в docs/audits/atlas-full-system-audit исходной рабочей копии; кандидаты и evidence не переписывались.

| ID | Результат в 2.5.1 | Проверка / остаток |
|---|---|---|
|001 IPC rejection завершает service|Client-scoped reject + bounded original deadline|12 broker tests; installed unauthorized-client test нужен|
|002 late network после exhausted retry|Epoch re-arms budget при сохранном intent|30/120/600s unit scenarios; physical network NOT RUN|
|003 refresh владеет reconnect|Удалён прямой connect, coordinator остаётся единственным владельцем|3 refresh +4 retry tests|
|004 reset selector installed2.4.2|Наследованы candidate selector/reconciliation fixes|2 selector +4 repository tests; installed final SHA обязателен|
|005 нет exact-SHA installed acceptance|CI собирает/подписывает конкретный PR head и проверяет тот же artifact|Открытый release gate до зелёного CI|
|006 IPC latency/busy|Добавлены request ID, queue/reply/core timing|Измерений workload/p95 ещё нет; частично открыто|
|007 startup visibility теряет restore intent|Durable intent + isolated save_visibility|Crash-window storage test,10 coordinator tests|
|008 refresh failures вытесняют backups|Operational status без configuration checkpoint|100 ошибок, backup сохранён|
|009 WinCred/SQLite crash consistency|Durable compensation journal + surfaced failures|15 storage tests; real power-loss NOT RUN|
|010 correlated control endpoints|Второй независимый HTTPS endpoint|10 latency tests; provider-wide outage NOT RUN|
|011 повторный evidence и ring serialization|Consecutive errors coalesced, append serializes delta|86400-error fixture,4 history tests; disk endurance NOT RUN|
|012 startup configure error скрыт|startupStatus/error в snapshot и UI|Registration helpers + coordinator; installed matrix CI|
|013 зависший uplink worker|Stale=unknown, один bounded replacement, global cap|8 monitor tests; две вечные native blocks — limitation|
|014 Unknown TUN marker ambiguity|liveOwnershipConfirmed/classificationMeaning|Conservative recovery не ослаблен; marker сам не live ownership|
|015 hidden CEF/polling budget|Наследованы isolation/read-state/window-startup fixes|CPU/memory installed benchmark не измерен; открыто|
|016 timeline/privacy|UUID session/build/monotonic timing добавлены, redaction сохранена|Публичный privacy export ещё не реализован; частично открыто|
|017 uninstall loaded hives only|ProfileList + private RAII offline hive inspection|3 uninstall tests; реальный multi-user offline acceptance NOT RUN|

Блокирующие доказательства: clean signed artifact, полный backend и installed acceptance на финальном SHA. 006/015/016 остаются явно открытыми улучшениями, а не объявляются решёнными из-за отсутствия воспроизведения.
