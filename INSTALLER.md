# Atlas Alpha 2.0.1 — установщик

**[Скачать ZIP с EXE и подписью](https://github.com/snaps-creator/atlas/releases/download/v2.0.1-alpha.1/Atlas-2.0.1-alpha.1-Windows-x64.zip)**

Распакуйте архив и запустите `Atlas Alpha 2.0.1 Setup.exe`.

В архиве:
- `Atlas Alpha 2.0.1 Setup.exe` — Windows x64, 162232261 байт.
- `Atlas Alpha 2.0.1 Setup.exe.sig` — подпись обновлений.
- `SHA256SUMS.txt` — контрольная сумма EXE.

SHA-256 EXE: `4b7adc679456f43b7c8c193ac1b2f8eb384fbb923995ee5b1442a0381a411a68`.
Это проверенная сборка коммита `ff4d286313e826dc22c335376a3cff5b314aacdc`, тот же файл находится в корне репозитория. Подпись проверяется командой:

```powershell
node scripts/verify-installer.cjs "Atlas Alpha 2.0.1 Setup.exe"
```

`.sig` — подпись обновлений Tauri, не Windows Authenticode.

ZIP из Releases содержит полный EXE и не зависит от Git LFS. В отличие от него, «Code → Download ZIP» упаковывает исходники и требует включённой настройки репозитория «Include Git LFS objects in archives», иначе EXE будет текстовым указателем размером 134 байта. После клонирования Git используйте `git lfs pull`.

Технические документы и отчёты находятся в [docs](docs/).