using System.IO;
using System.Linq;
using System.Windows;
using WeChatSendGuard.App.Services;
using WeChatSendGuard.Core.Configuration;

namespace WeChatSendGuard.App;

public partial class App : System.Windows.Application
{
    private AppServices? _services;

    protected override async void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);
        ShutdownMode = ShutdownMode.OnExplicitShutdown;

        var isSilent = e.Args.Any(arg => string.Equals(arg, "--silent", StringComparison.OrdinalIgnoreCase)
                                      || string.Equals(arg, "--startup", StringComparison.OrdinalIgnoreCase)
                                      || string.Equals(arg, "--background", StringComparison.OrdinalIgnoreCase));

        var settingsPath = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "WeChatSendGuard",
            "settings.json");

        var settingsStore = new FileSettingsStore(settingsPath);
        var settings = await settingsStore.LoadAsync();
        _services = await AppServices.CreateAsync(settings, settingsStore);
        _services.Start(openSettingsOnStartup: !isSilent);
    }

    protected override void OnExit(ExitEventArgs e)
    {
        _services?.Dispose();
        base.OnExit(e);
    }
}
