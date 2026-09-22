# Обновления Atlas через GitHub Releases

Текущая версия: **Beta 17.2** (`1.0.0-beta.17.2`).

Установщик в корне — `Atlas Beta 17.2 Setup.exe`; рядом находится соответствующая
подпись `Atlas Beta 17.2 Setup.exe.sig`. Старые установщики и подписи удаляются при
подготовке новой версии. Проверка пары файлов:

```powershell
node scripts/verify-installer.cjs
```

Подпись updater проверяется публичным ключом из `src-tauri/tauri.conf.json`.
Она не является Authenticode-подписью издателя Windows.

## Выпуск

1. Синхронно изменить версию в package.json, package-lock.json, Cargo.toml,
   Cargo.lock и tauri.conf.json.
2. Создать PR. Check pull request выполняет тесты и проверяет сборку интерфейса.
   Build signed PR installer для веток этого репозитория собирает и подписывает
   установщик; форки не получают доступ к ключу.
3. Забрать артефакт `atlas-signed-installer`, проверить подпись и добавить актуальные
   EXE и SIG в PR. Закрытый ключ никогда не скачивается из CI.
4. После merge в main workflow Publish Atlas update собирает подписанный Release
   с установщиком, SIG и latest.json. Merge сам по себе не означает завершение выпуска.

Для подписи в GitHub Actions используются существующие секреты
TAURI_SIGNING_PRIVATE_KEY и TAURI_SIGNING_PRIVATE_KEY_PASSWORD. Публичный ключ
нельзя менять без отдельного плана миграции установленных клиентов.

Пользователь скачивает актуальный установщик из корня main после merge либо из
GitHub Releases после успешного выпуска. Встроенный updater обращается к
`releases/latest/download/latest.json`, а не к EXE в исходниках.

Atlas проверяет обновления при запуске и каждые шесть часов. Ручная проверка —
в разделе «Обновления»; установка начинается по кнопке пользователя. Для выпуска
следующей версии нужно повысить номер: уже опубликованный tag повторно не выпускается.

Состав диагностики Beta 17.2: [INCIDENT-DIAGNOSTICS.md](INCIDENT-DIAGNOSTICS.md).
Результаты сетевого аудита и незакрытые вопросы: [NETWORK-AUDIT.md](NETWORK-AUDIT.md).
