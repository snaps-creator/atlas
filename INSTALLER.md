# Установка Atlas Alpha 2.1.1

1. Скачайте Code → Download ZIP с GitHub.
2. Распакуйте архив полностью.
3. Запустите **Install Atlas.exe**. Папка **installer** должна лежать рядом.

Это настоящий Windows x64 EXE. Он собирает установщик из двух локальных файлов, проверяет встроенные размер и SHA-256, затем открывает обычный мастер установки Atlas. Интернет, Git, Node.js и Git LFS для установки не нужны. Используется встроенный в Windows 10/11 .NET Framework 4.x. При повреждённой или неполной загрузке запуск отменяется с сообщением. Отмена UAC также не запускает установку.

Полный установщик Alpha 2.1.1 имеет размер 161966633 байт и SHA-256 `c20c2494bcab61e7f9050ee86d5a838306230fcf32899e79cfcc93e90c0c9e01`. Подпись обновлений находится в `installer/atlas-setup.exe.sig`. Источник — PR #38, коммит `982e039`, подписанная CI-сборка [36430674287](https://github.com/snaps-creator/atlas/actions/runs/36430674287).

Запускатель и данные хранятся обычными Git-файлами, каждый меньше 100 МиБ. ZIP исходников содержит реальные байты независимо от настройки «Include Git LFS objects in archives». Старые ZIP нужно скачать заново.

`SHA256SUMS.txt` содержит хэш запускателя. Windows Authenticode отсутствует: Windows может показывать неизвестного издателя. Подпись обновлений Tauri не является Authenticode.

Проверки без установки:

```powershell
node scripts/verify-offline-installer.cjs
./scripts/test-offline-installer.ps1
```

Обновление комплекта из нового подписанного установщика:

```powershell
./scripts/build-offline-installer.ps1 -Installer path/to/signed-setup.exe
```

После обновления нужно пересчитать SHA256SUMS.txt, закоммитить Install Atlas.exe и весь installer/, скачать ZIP ветки с GitHub и повторить проверки. `--verify` проверяет сборку данных и SHA-256 без запуска мастера установки.
