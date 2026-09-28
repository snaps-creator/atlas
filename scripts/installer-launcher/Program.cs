using System;
using System.Diagnostics;
using System.IO;
using System.Security.Cryptography;
using System.Windows.Forms;

internal static class Program
{
    [STAThread]
    private static int Main(string[] args)
    {
        bool verifyOnly = args.Length == 1 && args[0] == "--verify";
        if (args.Length != 0 && !verifyOnly) return 2;
        string work = Path.Combine(Path.GetTempPath(), "Atlas-Installer-" + Guid.NewGuid().ToString("N"));
        Form progress = null;
        try
        {
            if (!verifyOnly)
            {
                Application.EnableVisualStyles();
                progress = new Form { Text = "Установка Atlas", Width = 440, Height = 130,
                    StartPosition = FormStartPosition.CenterScreen, FormBorderStyle = FormBorderStyle.FixedDialog,
                    ControlBox = false };
                progress.Controls.Add(new Label { Text = "Подготовка и проверка установщика Atlas…", AutoSize = true, Left = 24, Top = 30 });
                progress.Show();
                Application.DoEvents();
            }
            string source = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "installer");
            // Names, size and digest are compiled in, never trusted from adjacent metadata.
            for (int i = 1; i <= Payload.Parts; i++)
                if (!File.Exists(Path.Combine(source, "atlas-setup.part" + i)))
                    throw new IOException("Не найдены файлы installer. Распакуйте весь ZIP, сохранив папку installer рядом с Install Atlas.exe.");
            Directory.CreateDirectory(work);
            string exe = Path.Combine(work, "Atlas.Setup.exe");
            using (var output = new FileStream(exe, FileMode.CreateNew, FileAccess.Write, FileShare.None))
            {
                byte[] buffer = new byte[1024 * 1024];
                long total = 0;
                for (int i = 1; i <= Payload.Parts; i++)
                    using (var input = new FileStream(Path.Combine(source, "atlas-setup.part" + i), FileMode.Open, FileAccess.Read, FileShare.Read))
                    {
                        int read;
                        while ((read = input.Read(buffer, 0, buffer.Length)) > 0)
                        {
                            total += read;
                            if (total > Payload.Size) throw new IOException("Неверный размер файлов установщика. Скачайте ZIP заново.");
                            output.Write(buffer, 0, read);
                            if (progress != null) Application.DoEvents();
                        }
                    }
                if (total != Payload.Size) throw new IOException("Установщик скачан не полностью. Скачайте ZIP заново.");
            }
            // Keep a read handle open through execution so the verified bytes cannot be rewritten.
            using (var verified = new FileStream(exe, FileMode.Open, FileAccess.Read, FileShare.Read))
            using (var sha = SHA256.Create())
            {
                string actual = BitConverter.ToString(sha.ComputeHash(verified)).Replace("-", "").ToLowerInvariant();
                if (actual != Payload.Sha256) throw new IOException("Контрольная сумма установщика не совпала. Запуск отменён. Скачайте ZIP заново.");
                if (progress != null) { progress.Close(); progress = null; }
                if (verifyOnly) return 0;
                using (var process = Process.Start(new ProcessStartInfo(exe) { UseShellExecute = true, WorkingDirectory = work }))
                {
                    if (process == null) throw new IOException("Не удалось запустить установщик.");
                    process.WaitForExit();
                    return process.ExitCode;
                }
            }
        }
        catch (Exception error)
        {
            if (!verifyOnly) MessageBox.Show(error.Message, "Установка Atlas", MessageBoxButtons.OK, MessageBoxIcon.Error);
            return 1;
        }
        finally
        {
            if (progress != null) progress.Dispose();
            // Only this launcher's randomly named temporary directory is removed.
            try { if (Directory.Exists(work)) Directory.Delete(work, true); } catch (IOException) { } catch (UnauthorizedAccessException) { }
        }
    }
}
