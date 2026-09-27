# Atlas Alpha 2.0.1

Скачайте готовые файлы напрямую:

- [Установщик Windows x64 (.exe)](https://github.com/snaps-creator/atlas/releases/download/v2.0.1-alpha.1/Atlas_2.0.1-alpha.1_x64-setup.exe)
- [Подпись обновлений (.sig)](https://github.com/snaps-creator/atlas/releases/download/v2.0.1-alpha.1/Atlas_2.0.1-alpha.1_x64-setup.exe.sig)

GitHub «Code → Download ZIP» скачивает исходники. EXE и его подпись исключены из этого архива: без этого GitHub может выдавать вместо EXE текстовый указатель Git LFS размером 134 байта. Такой файл не запускается в Windows. Для установки используйте ссылку выше; размер настоящего EXE — 162206117 байт. В старых скачанных ZIP указатель останется — скачайте настоящий EXE отдельно.

Установщик: `Atlas Alpha 2.0.1 Setup.exe` (Windows x64).
Подпись обновлений: `Atlas Alpha 2.0.1 Setup.exe.sig`.

EXE хранится через Git LFS. После клонирования выполните `git lfs pull`.
Готовый файл без Git LFS можно скачать из [релиза v2.0.1-alpha.1](https://github.com/snaps-creator/atlas/releases/tag/v2.0.1-alpha.1).

Файлы взяты из успешной сборки релиза коммита `47a69be7cbd9a2d0ab02670041c7958ead822533`, включающего исправления карточек сервисов и потокового пинга.

- Размер EXE: 162206117 байт.
- SHA-256: `7798e6df4c70b519bb8a6638b56dadbc56417f1e09566a51bbf6e8bc067247b8`.
- Подпись `.sig` проверена командой `node scripts/verify-installer.cjs "Atlas Alpha 2.0.1 Setup.exe"` по публичному ключу приложения.

`.sig` — криптографическая подпись для механизма обновлений Tauri; она не заменяет издательскую подпись Windows Authenticode.
