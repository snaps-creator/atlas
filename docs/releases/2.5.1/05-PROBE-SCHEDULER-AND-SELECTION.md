# Плановые проверки и выбор

service Recovery.schedule проверяет unknown/stale резервные узлы общего пула при работающей сессии, включая ручной выбор active node. Предыдущая зависимость от трёх slow samples убрана. Планирование не требует ручного ping.

Два узла за batch, двухсекундный минимальный interval; порядок по самому старому attempt, unknown сначала. Успешные/remote-failed samples имеют TTL autoTestIntervalSeconds (по умолчанию300 секунд); local busy/error не объявляет сервер мёртвым и получает retry delay30 секунд. Pool более100 узлов не starvation: regression fixture содержит126 unknown. Смена generation/source/network invalidates queued observations. Рабочие probes ограничены существующими query/probe mechanisms; ручной batch использует до6 workers.

DISPLAY_URL сохраняет привычное HTTP-измерение. Для подтверждения fallback применяется независимый HTTPS Google generate_204 наряду с Cloudflare; ожидается точный статус204. Внешний endpoint может быть недоступен в конкретной сети: timeout не доказывает поломку всех VPN-серверов.

Измерение ping не пишет settings.selected, AUTO selection или route_revision. Автоматический recovery может переключить подтверждённо неработающий AUTO/FAILOVER route независимо от измерения. Здоровое соединение не оптимизируется при autoOptimize=false. Явная оптимизация требует3 slow samples, повторного улучшения, абсолютного/относительного порога и60 секунд удержания; ручной pin не оптимизируется. Поэтому результат измерения и действие recovery могут совпадать по времени, но имеют разных владельцев.

Проверено unit/local proxy fixtures. Полный fairness/memory бюджет большого installed пула и одновременный manual/background batch требуют CI/изолированной нагрузки.
