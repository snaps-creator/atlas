using System;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Threading.Tasks;
using System.Web.Script.Serialization;
using System.Windows.Forms;

internal static class Program
{
    [STAThread]
    static int Main(string[] args)
    {
        bool verify = args.Length == 1 && args[0] == "--verify";
        if (args.Length != 0 && !verify) return 2;
        string work = Path.Combine(Path.GetTempPath(), "Atlas-Setup-" + Guid.NewGuid().ToString("N"));
        try
        {
            Directory.CreateDirectory(work);
            string helper = Path.Combine(work, "AtlasSetupHost.exe");
            using (var source = Assembly.GetExecutingAssembly().GetManifestResourceStream("AtlasSetupHost.exe"))
            using (var output = new FileStream(helper, FileMode.CreateNew, FileAccess.Write, FileShare.None))
                source.CopyTo(output);
            // Keep the embedded verifier immutable until its process exits.
            using (var pinned = new FileStream(helper, FileMode.Open, FileAccess.Read, FileShare.Read))
            using (var sha = SHA256.Create())
            {
                string digest = BitConverter.ToString(sha.ComputeHash(pinned)).Replace("-", "").ToLowerInvariant();
                if (digest != BootstrapPayload.Sha256) throw new IOException("Повреждён установщик Atlas. Скачайте Install Atlas.exe заново.");
                if (verify) return 0;
                Application.EnableVisualStyles();
                Application.SetCompatibleTextRenderingDefault(false);
                using (var window = new SetupWindow(helper, work))
                {
                    Application.Run(window);
                    return window.Result;
                }
            }
        }
        catch (Exception error)
        {
            if (!verify) MessageBox.Show(error.Message, "Установка Atlas", MessageBoxButtons.OK, MessageBoxIcon.Error);
            return 1;
        }
        finally
        {
            try { Directory.Delete(work, true); } catch (IOException) { } catch (UnauthorizedAccessException) { }
        }
    }
}

internal sealed class SetupWindow : Form
{
    readonly string helper, work, cancelFile;
    readonly Label status = new Label { AutoSize = false, Left = 24, Top = 24, Width = 470, Height = 58 };
    readonly ProgressBar progress = new ProgressBar { Left = 24, Top = 88, Width = 470, Height = 18, Style = ProgressBarStyle.Marquee };
    readonly Button cancel = new Button { Left = 394, Top = 126, Width = 100, Height = 30, Text = "Отмена" };
    bool finished, installing, cancelled;
    public int Result { get; private set; }
    public SetupWindow(string helperPath, string directory)
    {
        helper = helperPath; work = directory; cancelFile = Path.Combine(work, "cancel");
        Text = "Установка Atlas"; ClientSize = new System.Drawing.Size(518, 180);
        AutoScaleMode = AutoScaleMode.Dpi; StartPosition = FormStartPosition.CenterScreen;
        FormBorderStyle = FormBorderStyle.FixedDialog; MaximizeBox = false;
        status.Text = "Проверяем доступную версию Atlas…";
        Controls.Add(status); Controls.Add(progress); Controls.Add(cancel);
        cancel.Click += delegate { CancelDownload(); };
        FormClosing += delegate(object sender, FormClosingEventArgs e) { if (!finished) { e.Cancel = true; CancelDownload(); } };
        Shown += async delegate { await RunInstaller(); };
    }
    void CancelDownload()
    {
        if (installing || cancelled) return;
        cancelled = true; cancel.Enabled = false;
        status.Text = "Отменяем загрузку…";
        try { File.WriteAllText(cancelFile, "cancel"); } catch (IOException) { } catch (UnauthorizedAccessException) { }
    }
    void UpdateStage(string stage, string message)
    {
        if (stage == "installing")
        {
            installing = true; cancel.Enabled = false;
            status.Text = "Пакет проверен. Подтвердите запрос Windows\r\nи завершите установку в открывшемся мастере.";
        }
        else if (!cancelled && stage == "downloading") status.Text = "Скачиваем полный пакет Atlas " + message + ".\r\nПосле загрузки проверим подпись и целостность.";
    }
    async Task RunInstaller()
    {
        string failure = null;
        try
        {
            Result = await Task.Run(() => {
                var start = new ProcessStartInfo(helper, "--download-setup \"" + cancelFile + "\"") {
                    UseShellExecute = false, CreateNoWindow = true, WorkingDirectory = work,
                    RedirectStandardOutput = true, RedirectStandardError = true, StandardOutputEncoding = Encoding.UTF8,
                    StandardErrorEncoding = Encoding.UTF8
                };
                using (var process = new Process { StartInfo = start })
                {
                    process.ErrorDataReceived += delegate { }; // Drain stderr without logging user data.
                    if (!process.Start()) throw new IOException("Не удалось запустить установщик Atlas.");
                    process.BeginErrorReadLine();
                    var json = new JavaScriptSerializer { MaxJsonLength = 8192 };
                    string line;
                    while ((line = process.StandardOutput.ReadLine()) != null)
                    {
                        if (line.Length > 8192) continue;
                        var value = json.DeserializeObject(line) as System.Collections.Generic.Dictionary<string, object>;
                        if (value == null || !value.ContainsKey("stage")) continue;
                        string stage = Convert.ToString(value["stage"]);
                        string message = value.ContainsKey("message") ? Convert.ToString(value["message"]) : "";
                        if (stage == "error") failure = message;
                        BeginInvoke(new Action(() => UpdateStage(stage, message)));
                    }
                    process.WaitForExit();
                    return process.ExitCode;
                }
            });
            if (Result != 0 && !cancelled)
                MessageBox.Show(this, "Установка не завершена. Можно повторить запуск или скачать полный пакет из GitHub Releases.\r\n\r\n" + failure,
                    "Установка Atlas", MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
        catch (Exception error)
        {
            Result = 1;
            MessageBox.Show(this, error.Message, "Установка Atlas", MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
        finally { finished = true; Close(); }
    }
}
