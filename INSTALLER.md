# Atlas Alpha 2.0.1

Установщик: `Atlas Alpha 2.0.1 Setup.exe` (Windows x64).
Подпись обновлений: `Atlas Alpha 2.0.1 Setup.exe.sig`.

EXE хранится через Git LFS. После клонирования выполните `git lfs pull`.
Готовый файл без Git LFS можно скачать из [релиза v2.0.1-alpha.1](https://github.com/snaps-creator/atlas/releases/tag/v2.0.1-alpha.1).

Файлы взяты из успешной сборки релиза коммита `47a69be7cbd9a2d0ab02670041c7958ead822533`, включающего исправления карточек сервисов и потокового пинга.

- Размер EXE: 162206117 байт.
- SHA-256: `7798e6df4c70b519bb8a6638b56dadbc56417f1e09566a51bbf6e8bc067247b8`.
- Подпись `.sig` проверена командой `node scripts/verify-installer.cjs "Atlas Alpha 2.0.1 Setup.exe"` по публичному ключу приложения.

`.sig` — криптографическая подпись для механизма обновлений Tauri; она не заменяет издательскую подпись Windows Authenticode.
