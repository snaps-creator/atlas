# Фактические проверки

Дата локального запуска:2026-10-10. Рабочая установка не затронута. Финальный SHA/CI дополнительно фиксируются в release readiness и PR review.

| Проверка | Результат |
|---|---|
|npm ci|PASS,119 packages,0 vulnerabilities|
|npm test|PASS,51 тест/12 файлов|
|npm run build|PASS; existing chunk >500kB warning|
|node --test scripts/*.check.cjs|PASS,37 тестов на момент изменения distribution; повторяется после final edit|
|cargo test --locked --lib --no-run|PASS,2.5.1 native compile,246 tests discovered|
|storage/model/settings_write/refresh/retry/startup coordinator/network/history/selection/latency/broker/config/query/published state|PASS,98 тестов|
|model::repository/background_probe/core::selector_state|PASS,4+2+2 теста|
|startup helpers|PASS,4 теста|
|AtlasUpdater uninstall_startup|PASS,3 теста,уникальные временные HKCU fixtures|
|Полный backend/restricted-process|NOT RUN локально; обязательный CI|
|Clean signed installer|PENDING CI; локальная unsigned сборка не заменяет signature acceptance|
|Installed fresh/upgrade2.4.2/upgrade2.4.3/reinstall/rollback/uninstall|PENDING CI,только одноразовый Windows runner|
|Реальный reboot/power loss/Wi-Fi switching|NOT RUN|

Sandbox блокировал один WinCred fixture (NoStorageAccess1312) и registry fixture (access denied5). После разрешённого исполнения за пределами sandbox те же тесты прошли; секреты и реальные startup entries пользователя не использовались. Фильтры node_repository/probe_engine дали0 тестов и не учитываются как PASS coverage; правильные repository/background_probe фильтры выполнены отдельно.

Логи локальных запусков находятся в ignored temp-*.log в worktree и не включают пользовательские подписки. CI failures не скрываются и continue-on-error не добавлен.
