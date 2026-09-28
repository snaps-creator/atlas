# Atlas Alpha 2.0.1 — установщик

**[Скачать ZIP с EXE и подписью](https://github.com/snaps-creator/atlas/releases/download/v2.0.1-alpha.1/Atlas-2.0.1-New-Design-Windows-x64.zip)**

Распакуйте архив и запустите `Atlas Alpha 2.0.1 Setup.exe`.

В архиве:
- `Atlas Alpha 2.0.1 Setup.exe` — Windows x64, 161972950 байт.
- `Atlas Alpha 2.0.1 Setup.exe.sig` — подпись обновлений.
- `SHA256SUMS.txt` — контрольная сумма EXE.

SHA-256 EXE: `1a8820c2f85a9d5bb04c33d69bc5d90c91480ac3d665fbcb6a2c77f971a83085`.
Это проверенная сборка коммита `024103d`, тот же файл находится в корне репозитория. Подпись проверяется командой:

```powershell
node scripts/verify-installer.cjs "Atlas Alpha 2.0.1 Setup.exe"
```

`.sig` — подпись обновлений Tauri, не Windows Authenticode.

ZIP из Releases содержит полный EXE и не зависит от Git LFS. В отличие от него, «Code → Download ZIP» упаковывает исходники и требует включённой настройки репозитория «Include Git LFS objects in archives», иначе EXE будет текстовым указателем размером 134 байта. После клонирования Git используйте `git lfs pull`.

Технические документы и отчёты находятся в [docs](docs/).