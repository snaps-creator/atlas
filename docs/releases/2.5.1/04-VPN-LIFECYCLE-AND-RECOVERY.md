# VPN intent, lifecycle и recovery

ConnectionIntent — текущая команда пользователя. wasConnected/userDisconnected — durable restore intent. Intent сохраняется до core.start, переживает visibility, ошибку подключения и crash window. Только явный Disconnect сбрасывает его; coordinator учитывает launch reason и startup policy. Manual restart получает свежие аргументы вместо унаследованного --autostart.

Состояния desktop: Initializing, Disconnected, Connecting, Connected, ProtectedPause, WaitingForNetwork, Error, Stopping, CleanupError. UI может отменить ожидающее подключение. ProtectedPause восстанавливается существующим service HealthState; desktop не создаёт второй цикл, пока служба отвечает. После потери службы desktop применяет bounded ConnectionRetry: 3,6,12,24,48 секунд, максимум пять неудач. Подтверждённая смена uplink после 30/120/600 секунд восстанавливает budget, если intent ещё включён. Refresh источника не сбрасывает этот budget и не вызывает connect самостоятельно.

Read-only uplink monitor использует native Windows adapters/default route/DNS/WLAN, двухсекундный debounce и epoch. Неизвестная или stale observation не считается доказательством offline. Запросы выполняются вне App lock. Через 15 секунд возможен один replacement worker; максимум два retained workers на monitor и четыре native workers глобально. Два навсегда зависших Windows API call остаются ограничением, а не поводом бесконечно плодить потоки.

IPC проверяет image/PID, rejects unauthorized client без завершения live session и не продлевает исходный admission deadline (3 секунды без configuration,30 секунд configured). SCM stop остаётся приоритетным. Ошибка клиента при сохранном guard/session не является общей фатальной ошибкой службы. Независимый cleanup observer контролирует lifetime владельца. Query generation отклоняет старые результаты.

Installed startup/late-network/реальная смена Wi-Fi должны подтвердиться на одноразовом runner. На пользовательской машине VPN, DNS, routes, TUN, WFP и установленная служба не изменялись.
