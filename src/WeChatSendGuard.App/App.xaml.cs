using System.IO;
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

        var settingsPath = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "WeChatSendGuard",
            "settings.json");
        var isFirstLaunch = !File.Exists(settingsPath);
        var settingsStore = new FileSettingsStore(settingsPath);
        var settings = await settingsStore.LoadAsync();
        _services = await AppServices.CreateAsync(settings, settingsStore);
        _services.Start(openSettingsOnStartup: isFirstLaunch);
    }

    protected override void OnExit(ExitEventArgs e)
    {
        _services?.Dispose();
        base.OnExit(e);
    }
}
