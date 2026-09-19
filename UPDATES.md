# Обновления Atlas через GitHub Releases

Текущая версия приложения: **Beta v1** (`1.0.0-beta.4`).

## Однократная настройка GitHub

Закрытый ключ создан локально и не входит в Git:

```text
C:\Users\nikid\.tauri\atlas-updater.key
C:\Users\nikid\.tauri\atlas-updater.password.txt
```

В репозитории GitHub откройте **Settings → Secrets and variables → Actions**,
создайте секрет `TAURI_SIGNING_PRIVATE_KEY` и вставьте в него полное содержимое
файла `atlas-updater.key`. Создайте секрет
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` с содержимым файла
`atlas-updater.password.txt`. Публичный ключ уже находится в
`src-tauri/tauri.conf.json`.

Репозиторий и его Releases должны быть публично доступны: клиент обращается к
`releases/latest/download/latest.json` без GitHub-токена.

## Публикация обновления

1. Обновите SemVer одновременно в `package.json`, `src-tauri/Cargo.toml` и
   `src-tauri/tauri.conf.json`.
2. Обновите видимое название версии в `src/main.tsx`.
3. Создайте Pull Request и дождитесь успешной проверки **Check pull request**.
4. Выполните merge в `main`. Публикация начнётся автоматически после merge.

Workflow запускает тесты, собирает NSIS-установщик, подписывает пакет обновления
и публикует GitHub Release вместе с `latest.json` и `.sig`. Закрытый ключ и
GitHub-токен в приложение не встраиваются.

Подпись updater подтверждает целостность обновления внутри Atlas. Для удаления
сообщения Windows «Неизвестный издатель» нужен отдельный Authenticode-сертификат
издателя; ключ updater его не заменяет.

Pull Request запускает только тесты и проверку production-сборки. У него нет
права записи в репозиторий, поэтому он не создаёт tag или Release. Перед каждым
следующим merge с публикацией обязательно увеличьте версию: повторно использовать
существующий tag workflow не позволит.
