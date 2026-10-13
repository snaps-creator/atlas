# Единые источники и серверы

Settings.subscriptions — канонический список Subscription. SubscriptionSource различает URL и VLESS только как способ получения данных. Settings.servers() собирает все узлы без зависимости от фильтра интерфейса. Старое представление списка сохранено как совместимость сериализации, но не как параллельное хранилище.

model::repository.normalize формирует source-scoped node ID и metadata atlas: sourceId, sourceName, sourceType, nodeId, stableIdentity, protocol, transport, displayName. Одинаковые подписи у разных источников не объединяются. Идентичность исходит из реальных параметров транспорта; display metadata исключаются из runtime config.

reconcile_references сохраняет выбор и избранное только в исходном источнике. При ротации параметров допустим лишь однозначный remap. Неоднозначность не угадывается и не переносится на другой источник. Кэш latency invalidates по generation/identity, а не по видимому имени.

Обновление URL изменяет свой источник и применяет необходимый config через существующий coordinator. VLESS не показывает URL refresh/User-Agent. Название можно изменить без смены source ID. Удаление одного источника сохраняет ссылки других; удаление active node использует существующую reconciliation policy. Операция источника не запускает дополнительный reconnect owner.
