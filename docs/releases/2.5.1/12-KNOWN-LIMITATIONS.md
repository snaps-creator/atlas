# Known limitations

- Physical reboot/power-loss acceptance не выполнен. Durable journal/process-interruption тесты не эквивалентны аварии питания.
- Реальное переключение Wi-Fi/VPN у пользователя не тестировалось. Native monitor bounded replacement не может отменить зависший Windows API call; если оба retained calls зависнут навсегда, fresh observations потребуют restart.
- Большой installed server pool/hidden CEF p95 CPU/memory/IPC budget не измерены. Queue/reply/core instrumentation добавлена для дальнейшего evidence.
- Публичный privacy export не реализован; локальные адреса/adapter names могут присутствовать в отчёте.
- Synthetic offline NTUSER.DAT cleanup прошёл isolated legacy acceptance на f26dfe0 (Actions run38075294004); final-head matrix остаётся обязательной. Locked hive/logon race не воспроизведён.
- Authenticode signing не заявлен. Используется существующая подпись обновления Atlas/minisign.
- 2.4.3 не опубликована: upgrade baseline собирается из actual source commit49dfcde. Это проверка candidate2.4.3, а не утверждение о существующем Releases asset.
- Новое имя public asset не переименовывает historical releases или старые signed manifests. Existing update verification остаётся совместимым с manifest schema1/2 согласно native contract; подготовка нового релиза требует schema2.
- До полного final-SHA CI статус clean signed build/installed acceptance остаётся PENDING; release не готов только на основании local unit tests.
