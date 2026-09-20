# Atlas

[![Проверка PR](https://github.com/snaps-creator/atlas/actions/workflows/pr-check.yml/badge.svg)](https://github.com/snaps-creator/atlas/actions/workflows/pr-check.yml)
[![Публикация обновления](https://github.com/snaps-creator/atlas/actions/workflows/release.yml/badge.svg?branch=main)](https://github.com/snaps-creator/atlas/actions/workflows/release.yml)

Существующее Windows-приложение Atlas Beta v1 на React, TypeScript, Tauri 2, Rust и Mihomo.
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

Готовый установщик для пользователя находится в корне проекта: `Atlas Beta 14.1 Setup.exe`. Скрипт `scripts/build.ps1` автоматически включает текущую версию в название установщика.
Исходный bundle после сборки: `src-tauri/target/release/bundle/nsis/Atlas_1.0.0-beta.14.1_x64-setup.exe`.
Ядро и локальная GeoIP-база включены в проект. Контрольные суммы находятся в `src-tauri/resources`.
Дистрибутив пока не подписан издательским сертификатом, поэтому Windows показывает
«Неизвестный издатель» при установке. При обычном запуске Atlas этот запрос больше
не появляется: привилегированные сетевые операции выполняет установленная служба.

Автоматические обновления поставляются через подписанные GitHub Releases. Инструкция для выпуска новой версии: [UPDATES.md](UPDATES.md).

## Подключение

1. Добавьте HTTPS-подписку в разделе «Подписки».
2. Выберите сервер.
3. Используйте переключатель подключения. UAC требуется один раз при установке или
   обновлении системной сетевой службы, а не при каждом запуске и подключении.

Перед проверкой TUN отключите другой активный VPN. Сетевая приёмка восстановления после аварии ещё не завершена — см. STATUS.md и TUN-VALIDATION.md.

## Правила

Приложение + домен → приложение → точный домен → суффикс → ключевое слово → IP-сеть → маршрут по умолчанию.
Порядок записей YAML не определяет приоритет. TUN перехватывает трафик для применения
правил, а итоговый маршрут каждого соединения задают `proxy`, `direct`, `block` и
`default-route`. Для выборочного VPN можно использовать `default-route: direct`.

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
- `core.rs`, `broker.rs`, `service.rs`, `network_guard.rs`: ядро, системная служба, WFP.
- `rule_probe.rs`, `latency.rs`, `network_diagnostics.rs`: измерения и диагностика.
- `storage.rs`, `subscriptions.rs`: настройки и подписки.

Mihomo 1.19.31 и GeoIP распространяются с лицензиями в ресурсах. Исходники используемой версии Mihomo находятся в `third_party`.
