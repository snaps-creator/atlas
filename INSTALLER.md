# Atlas Alpha 2.0.1

Для каждого PR сборка **Build signed PR installer** создаёт скачиваемый артефакт `Atlas-Alpha-2.0.1-Windows-x64-PR-<номер>`. В ZIP находятся настоящий EXE, его подпись `.sig` и `SHA256SUMS.txt`. Это результат новой сборки коммита PR, а не Git LFS-указатель. Откройте проверку PR → Summary → Artifacts и скачайте этот архив.

Новая сборка PR #32: [ZIP с настоящим EXE, подписью и SHA256SUMS.txt](https://github.com/snaps-creator/atlas/actions/runs/36339996049/artifacts/10938952370). Артефакт Actions хранится 14 дней; EXE и подпись также сохранены в Git LFS в этой ветке.

Предыдущая опубликованная сборка:

- [Установщик Windows x64 (.exe)](https://github.com/snaps-creator/atlas/releases/download/v2.0.1-alpha.1/Atlas_2.0.1-alpha.1_x64-setup.exe)
- [Подпись обновлений (.sig)](https://github.com/snaps-creator/atlas/releases/download/v2.0.1-alpha.1/Atlas_2.0.1-alpha.1_x64-setup.exe.sig)

GitHub «Code → Download ZIP» скачивает исходники. EXE и его подпись исключены из этого архива: без этого GitHub может выдавать вместо EXE текстовый указатель Git LFS размером 134 байта. Такой файл не запускается в Windows. Для установки используйте ссылку выше; размер настоящего EXE — 162232261 байт. В старых скачанных ZIP указатель останется — скачайте настоящий EXE отдельно.

Установщик: `Atlas Alpha 2.0.1 Setup.exe` (Windows x64).
Подпись обновлений: `Atlas Alpha 2.0.1 Setup.exe.sig`.

EXE хранится через Git LFS. После клонирования выполните `git lfs pull`.
Новый файл без Git LFS доступен в артефакте PR по ссылке выше.

Файлы взяты из успешной подписанной сборки PR #32, коммит `ff4d286313e826dc22c335376a3cff5b314aacdc`.

- Размер EXE: 162232261 байт.
- SHA-256: `4b7adc679456f43b7c8c193ac1b2f8eb384fbb923995ee5b1442a0381a411a68`.
- Подпись `.sig` проверена командой `node scripts/verify-installer.cjs "Atlas Alpha 2.0.1 Setup.exe"` по публичному ключу приложения.

`.sig` — криптографическая подпись для механизма обновлений Tauri; она не заменяет издательскую подпись Windows Authenticode.
