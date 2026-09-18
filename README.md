# Атлас

Существующее Windows-приложение на React, TypeScript, Tauri 2, Rust и Mihomo.
Подробный статус реализации и подтверждённых проверок: [STATUS.md](STATUS.md).

## Сборка

Windows 10/11 x64, WebView2, Node.js, Rust MSVC, Microsoft C++ Build Tools и Windows SDK.

```powershell
npm ci
./scripts/fetch-core.ps1
npm test
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm run tauri build
```

Установщик: `src-tauri/target/release/bundle/nsis/Atlas_0.1.0_x64-setup.exe`.
Ядро и локальная GeoIP-база включены в проект. Контрольные суммы находятся в `src-tauri/resources`.
Дистрибутив не подписан издательским сертификатом.

## Подключение

1. Добавьте HTTPS-подписку в разделе «Подписки».
2. Выберите сервер.
3. Используйте переключатель подключения. TUN запрашивает UAC для сетевого помощника.

Перед проверкой TUN отключите другой активный VPN. Сетевая приёмка восстановления после аварии ещё не завершена — см. STATUS.md и TUN-VALIDATION.md.

## Правила

Приложение + домен → приложение → точный домен → суффикс → ключевое слово → IP-сеть → маршрут по умолчанию.
Порядок записей YAML не определяет приоритет. В TUN маршрут по умолчанию — VPN.

```yaml
version: 1
default-route: direct
rules:
  - process: Telegram.exe
    route: proxy
  - domain-suffix: openai.com
    route: proxy
  - domain-suffix: example.com
    route: block
```

Поддерживаются `process`, `domain`, `domain-suffix`, `domain-keyword`, `ip-cidr`, комбинация `process` + `domain`, необязательные `enabled` и `no-resolve`. Маршруты: `proxy`, `direct`, `block`.
YAML и таблица редактируют одну модель. Ошибки и неразрешённые конфликты не заменяют действующие настройки.

Проверка правил запускается только кнопкой пользователя. Для сайта выполняется ограниченный HTTPS-запрос; для приложения или сети анализируются наблюдаемые соединения. Связанные ошибки ядра хранятся в ограниченном буфере памяти. Предложения дополнительных доменов требуют выбора пользователя. HTTP 403/429 и отсутствие трафика не выдаются за успешную проверку.

## Данные

SQLite и 20 резервных состояний защищены Windows DPAPI. Исходное состояние перед новой логикой правил сохраняется отдельно. Ссылки подписок находятся в диспетчере учётных данных Windows. Переносимый экспорт правил не содержит подписок, паролей или абсолютных путей приложений.

## Основные модули

- `src/RulesPanel.tsx`, `YamlEditor.tsx`, `AppPicker.tsx`, `RuleChecks.tsx`: правила и их проверка.
- `src-tauri/src/rules.rs`, `portable.rs`: нормализация, приоритеты, импорт/экспорт.
- `core.rs`, `broker.rs`, `network_guard.rs`: ядро, привилегированный помощник, WFP.
- `rule_probe.rs`, `latency.rs`, `network_diagnostics.rs`: измерения и диагностика.
- `storage.rs`, `subscriptions.rs`: настройки и подписки.

Mihomo 1.19.31 и GeoIP распространяются с лицензиями в ресурсах. Исходники используемой версии Mihomo находятся в `third_party`.
