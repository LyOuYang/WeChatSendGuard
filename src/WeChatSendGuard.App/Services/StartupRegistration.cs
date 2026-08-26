using Microsoft.Win32;

namespace WeChatSendGuard.App.Services;

internal static class StartupRegistration
{
    private const string RunKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string ValueName = "WeChatSendGuard";

    public static bool Apply(bool enabled)
    {
        using var key = Registry.CurrentUser.OpenSubKey(RunKeyPath, writable: true)
            ?? throw new InvalidOperationException("Unable to open the current-user startup registry key.");
        if (!enabled)
        {
            key.DeleteValue(ValueName, throwOnMissingValue: false);
            return true;
        }

        var executable = Environment.ProcessPath;
        if (string.IsNullOrWhiteSpace(executable)
            || string.Equals(Path.GetFileNameWithoutExtension(executable), "dotnet", StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }

        key.SetValue(ValueName, $"\"{executable}\"", RegistryValueKind.String);
        return true;
    }
}
