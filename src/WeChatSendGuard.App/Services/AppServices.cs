using System.Security.Cryptography;
using System.Windows;
using System.Windows.Threading;
using WeChatSendGuard.App.Interop;
using WeChatSendGuard.Core.Configuration;
using WeChatSendGuard.Core.Guard;

namespace WeChatSendGuard.App.Services;

internal sealed class AppServices : IDisposable
{
    private readonly Dispatcher _dispatcher;
    private readonly ISettingsStore _settingsStore;
    private readonly FileAuditLog _auditLog;
    private readonly InputGateController _inputGate;
    private readonly TrayIconService _tray;
    private readonly SemaphoreSlim _settingsWriteLock = new(1, 1);
    private AppSettings _settings;
    private MainWindow? _settingsWindow;
    private bool _started;
    private bool _disposed;

    private AppServices(
        Dispatcher dispatcher,
        ISettingsStore settingsStore,
        FileAuditLog auditLog,
        WeixinContextMonitor contextMonitor,
        InputGateController inputGate,
        TrayIconService tray,
        AppSettings settings)
    {
        _dispatcher = dispatcher;
        _settingsStore = settingsStore;
        _auditLog = auditLog;
        ContextMonitor = contextMonitor;
        _inputGate = inputGate;
        _tray = tray;
        _settings = settings;
        ContextMonitor.ContextChanged += ContextMonitor_ContextChanged;
    }

    public WeixinContextMonitor ContextMonitor { get; }

    public AppSettings Settings => Volatile.Read(ref _settings);

    public static Task<AppServices> CreateAsync(AppSettings settings, ISettingsStore settingsStore)
    {
        var sanitizedSettings = SettingsValidator.Sanitize(settings);
        var dispatcher = Application.Current.Dispatcher;
        var contextMonitor = new WeixinContextMonitor(dispatcher);
        var auditDirectory = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "WeChatSendGuard",
            "logs");
        var auditLog = new FileAuditLog(auditDirectory, sanitizedSettings.LogRetentionDays);
        var injectionMarker = unchecked((nuint)RandomNumberGenerator.GetInt32(1, int.MaxValue));
        var keyboardHook = new LowLevelKeyboardHook(injectionMarker);
        var confirmationService = new WpfConfirmationService(dispatcher);
        var inputGate = new InputGateController(
            keyboardHook,
            contextMonitor,
            new SendGuardStateMachine(),
            new TemporaryBypassRegistry(),
            confirmationService,
            new InputInjector(injectionMarker),
            auditLog,
            sanitizedSettings);

        AppServices? services = null;
        var tray = new TrayIconService(
            () => services!.ShowSettings(),
            () => services!.ProtectCurrentGroup(),
            minutes => services!.GrantTemporaryBypass(minutes),
            enabled => services!.SetProtectionEnabled(enabled),
            () => services!.ShowStatus(),
            () => services!.Exit());
        services = new AppServices(dispatcher, settingsStore, auditLog, contextMonitor, inputGate, tray, sanitizedSettings);
        keyboardHook.CanRequestProtectCurrent = () => services!.ContextMonitor.CanHandleProtectCurrentShortcut;
        keyboardHook.ProtectCurrentRequested += services.ProtectCurrentGroupFromShortcut;
        return Task.FromResult(services);
    }

    public void Start()
    {
        if (_started || _disposed)
        {
            return;
        }

        _started = true;
        try
        {
            ContextMonitor.Start();
            _inputGate.Start();
            ApplyStartupRegistration();
            _tray.SetProtectionEnabled(_settings.Enabled);
            UpdateTrayStatus(ContextMonitor.Current);
        }
        catch (Exception ex) when (ex is System.ComponentModel.Win32Exception or InvalidOperationException)
        {
            _tray.SetStatus("WeChatSendGuard：保护未运行");
            _tray.ShowWarning("发送保护未启动", ex.Message);
        }

        if (_settings.ProtectedChats.Count == 0 && _settings.ExemptedChats.Count == 0)
        {
            _dispatcher.BeginInvoke(ShowSettings, DispatcherPriority.ApplicationIdle);
        }
    }

    public async Task ApplySettingsAsync(AppSettings settings)
    {
        await _settingsWriteLock.WaitAsync();
        try
        {
            await ApplySettingsCoreAsync(settings);
        }
        finally
        {
            _settingsWriteLock.Release();
        }
    }

    public void ShowSettings()
    {
        if (!_dispatcher.CheckAccess())
        {
            _dispatcher.BeginInvoke(ShowSettings);
            return;
        }

        if (_settingsWindow is null)
        {
            _settingsWindow = new MainWindow(this, _settings);
            _settingsWindow.Closed += (_, _) => _settingsWindow = null;
        }

        _settingsWindow.ShowSettings();
    }

    public void ProtectCurrentGroup()
    {
        if (!_dispatcher.CheckAccess())
        {
            _dispatcher.BeginInvoke(ProtectCurrentGroup);
            return;
        }

        AddCurrentChat(ContextMonitor.LastRecognizedWeixin);
    }

    private void ProtectCurrentGroupFromShortcut()
    {
        if (!_dispatcher.CheckAccess())
        {
            _dispatcher.BeginInvoke(ProtectCurrentGroupFromShortcut);
            return;
        }

        _ = ProtectCurrentGroupFromShortcutAsync();
    }

    private async Task ProtectCurrentGroupFromShortcutAsync()
    {
        try
        {
            AddCurrentChat(await ContextMonitor.RefreshNowAsync());
        }
        catch (Exception ex) when (ex is InvalidOperationException or System.ComponentModel.Win32Exception)
        {
            _tray.ShowWarning("无法加入当前会话", ex.Message);
        }
    }

    private void AddCurrentChat(ChatContext context)
    {
        if (!context.IsTrustedWeixin || !context.IsCompatibilityAvailable || !context.IsKnownChat || context.TargetKind is null || string.IsNullOrWhiteSpace(context.ChatTitle))
        {
            _tray.ShowWarning("无法加入当前会话", "请先在微信中打开一个可识别的群聊或联系人会话。 ");
            return;
        }

        var title = ChatTitleNormalizer.Normalize(context.ChatTitle);
        var isExemptionMode = _settings.RuleMode == RuleMode.ConfirmUnlessExcluded;
        var currentList = isExemptionMode ? _settings.ExemptedChats : _settings.ProtectedChats;
        if (currentList.Any(chat => chat.TargetKind == context.TargetKind && ProtectedChatMatcher.TitleMatches(chat, title)))
        {
            _tray.ShowInformation(isExemptionMode ? "会话已在免确认名单" : "会话已在保护名单", title);
            return;
        }

        var chat = new ProtectedChat { DisplayName = title, MatchTitle = title, TargetKind = context.TargetKind.Value };
        _ = AddChatAsync(chat, isExemptionMode);
    }

    public async Task<bool> AddChatAsync(ProtectedChat chat, bool exemptionList)
    {
        var sanitizedChat = SettingsValidator.SanitizeChat(chat);
        await _settingsWriteLock.WaitAsync();
        try
        {
            var current = Settings;
            var currentList = exemptionList ? current.ExemptedChats : current.ProtectedChats;
            if (string.IsNullOrEmpty(sanitizedChat.MatchTitle)
                || currentList.Any(existing => existing.TargetKind == sanitizedChat.TargetKind && ProtectedChatMatcher.TitleMatches(existing, sanitizedChat.MatchTitle)))
            {
                return false;
            }

            var next = exemptionList
                ? current with { ExemptedChats = [.. current.ExemptedChats, sanitizedChat] }
                : current with { ProtectedChats = [.. current.ProtectedChats, sanitizedChat] };
            await ApplySettingsCoreAsync(next);
            _tray.ShowInformation(exemptionList ? "会话已加入免确认名单" : "会话已加入保护名单", sanitizedChat.DisplayNameWithKind);
            return true;
        }
        finally
        {
            _settingsWriteLock.Release();
        }
    }

    public async Task RemoveChatAsync(Guid chatId, bool exemptionList)
    {
        await _settingsWriteLock.WaitAsync();
        try
        {
            var current = Settings;
            var next = exemptionList
                ? current with { ExemptedChats = current.ExemptedChats.Where(chat => chat.Id != chatId).ToList() }
                : current with { ProtectedChats = current.ProtectedChats.Where(chat => chat.Id != chatId).ToList() };
            await ApplySettingsCoreAsync(next);
        }
        finally
        {
            _settingsWriteLock.Release();
        }
    }

    public void GrantTemporaryBypass(int minutes)
    {
        if (!_dispatcher.CheckAccess())
        {
            _dispatcher.BeginInvoke(() => GrantTemporaryBypass(minutes));
            return;
        }

        if (_settings.RuleMode != RuleMode.ProtectListed || !_inputGate.TryGrantCurrentChatBypass(minutes, out var name))
        {
            _tray.ShowWarning("无法临时放行", "临时放行只适用于“名单内需要确认”模式的保护会话。 ");
            return;
        }

        _tray.ShowInformation("已临时放行", $"{name} 可直接发送 {minutes} 分钟。 ");
    }

    public void SetProtectionEnabled(bool enabled)
    {
        if (!_dispatcher.CheckAccess())
        {
            _dispatcher.BeginInvoke(() => SetProtectionEnabled(enabled));
            return;
        }

        _ = ApplySettingsAsync(_settings with { Enabled = enabled });
    }

    public void ShowStatus()
    {
        if (!_dispatcher.CheckAccess())
        {
            _dispatcher.BeginInvoke(ShowStatus);
            return;
        }

        var context = ContextMonitor.Current;
        var text = !_settings.Enabled
            ? "发送保护已暂停。"
            : context.RequiresElevation
                ? "微信以管理员权限运行，发送保护无法读取其窗口。请用普通权限重新启动微信。"
            : !context.IsTrustedWeixin
                ? "工具正在运行，等待微信成为前台窗口。"
                : !context.IsCompatibilityAvailable
                    ? "工具正在运行，但当前微信界面不可识别，保护未生效。"
                    : context.IsMessageEditorFocused
                        ? "发送保护已就绪。"
                        : "微信已在前台，请将光标放入消息输入框。";
        MessageBox.Show(text, "WeChatSendGuard 状态", MessageBoxButton.OK, MessageBoxImage.Information);
    }

    public void Exit()
    {
        if (!_dispatcher.CheckAccess())
        {
            _dispatcher.BeginInvoke(Exit);
            return;
        }

        Application.Current.Shutdown();
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        ContextMonitor.ContextChanged -= ContextMonitor_ContextChanged;
        _inputGate.Dispose();
        ContextMonitor.Dispose();
        _tray.Dispose();
        _auditLog.Dispose();
    }

    private void ContextMonitor_ContextChanged(object? sender, ChatContext context)
    {
        if (!_dispatcher.CheckAccess())
        {
            _dispatcher.BeginInvoke(() => UpdateTrayStatus(context));
            return;
        }

        UpdateTrayStatus(context);
    }

    private async Task ApplySettingsCoreAsync(AppSettings settings)
    {
        var sanitized = SettingsValidator.Sanitize(settings);
        await _settingsStore.SaveAsync(sanitized);
        Interlocked.Exchange(ref _settings, sanitized);
        _inputGate.UpdateSettings(sanitized);
        _auditLog.SetRetentionDays(sanitized.LogRetentionDays);
        ApplyStartupRegistration();
        _tray.SetProtectionEnabled(sanitized.Enabled);
        UpdateTrayStatus(ContextMonitor.Current);
    }

    private void UpdateTrayStatus(ChatContext context)
    {
        if (!_settings.Enabled)
        {
            _tray.SetStatus("WeChatSendGuard：发送保护已暂停");
            return;
        }

        if (context.RequiresElevation)
        {
            _tray.SetStatus("WeChatSendGuard：微信权限不兼容");
            return;
        }

        if (!context.IsTrustedWeixin)
        {
            _tray.SetStatus("WeChatSendGuard：等待微信前台");
            return;
        }

        if (!context.IsCompatibilityAvailable)
        {
            _tray.SetStatus("WeChatSendGuard：微信界面不可识别");
            return;
        }

        _tray.SetStatus(context.IsMessageEditorFocused
            ? "WeChatSendGuard：发送保护已就绪"
            : "WeChatSendGuard：等待消息输入框焦点");
    }

    private void ApplyStartupRegistration()
    {
        try
        {
            _ = StartupRegistration.Apply(_settings.StartWithWindows);
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or InvalidOperationException)
        {
            _tray.ShowWarning("无法设置开机启动", ex.Message);
        }
    }
}
