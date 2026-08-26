using System.Globalization;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Threading;
using Microsoft.Win32;
using WeChatSendGuard.App.Services;
using WeChatSendGuard.Core.Configuration;
using WeChatSendGuard.Core.Guard;

namespace WeChatSendGuard.App;

public partial class MainWindow : Window
{
    private static readonly TimeSpan AutoSaveDelay = TimeSpan.FromMilliseconds(400);

    private readonly AppServices _services;
    private readonly DispatcherTimer _autoSaveTimer;
    private AppSettings _settings;
    private bool _loadingSettings;

    internal MainWindow(AppServices services, AppSettings settings)
    {
        _services = services;
        _settings = SettingsValidator.Sanitize(settings);
        _loadingSettings = true;
        _autoSaveTimer = new DispatcherTimer { Interval = AutoSaveDelay };
        _autoSaveTimer.Tick += AutoSaveTimer_Tick;

        InitializeComponent();
        ConfigureChoices();
        LoadSettings(_settings);
        _services.ContextMonitor.ContextChanged += ContextMonitor_ContextChanged;
        Closing += (_, _) =>
        {
            _autoSaveTimer.Stop();
            _ = CommitSettingsAsync(allowInvalidNumbers: false);
        };
        Closed += (_, _) =>
        {
            _autoSaveTimer.Stop();
            _services.ContextMonitor.ContextChanged -= ContextMonitor_ContextChanged;
        };
    }

    public void ShowSettings()
    {
        if (!IsVisible)
        {
            Show();
        }

        if (WindowState == WindowState.Minimized)
        {
            WindowState = WindowState.Normal;
        }

        Activate();
        Topmost = true;
        Topmost = false;
    }

    private bool IsExemptionMode => _settings.RuleMode == RuleMode.ConfirmUnlessExcluded;

    private void ConfigureChoices()
    {
        ConfirmationModeComboBox.ItemsSource = new[]
        {
            new Choice<ConfirmationMode>(ConfirmationMode.Click, "单击确认"),
            new Choice<ConfirmationMode>(ConfirmationMode.Hold, "长按确认"),
            new Choice<ConfirmationMode>(ConfirmationMode.Phrase, "输入确认词"),
        };
        UnknownContextComboBox.ItemsSource = new[]
        {
            new Choice<UnknownContextBehavior>(UnknownContextBehavior.Confirm, "要求确认"),
            new Choice<UnknownContextBehavior>(UnknownContextBehavior.Block, "直接阻止"),
        };
        RuleModeComboBox.ItemsSource = new[]
        {
            new Choice<RuleMode>(RuleMode.ProtectListed, "保护名单模式：名单内会话需要确认"),
            new Choice<RuleMode>(RuleMode.ConfirmUnlessExcluded, "免确认名单模式：名单外会话需要确认"),
        };
    }

    private void LoadSettings(AppSettings settings)
    {
        _loadingSettings = true;
        try
        {
            _settings = SettingsValidator.Sanitize(settings);
            EnabledCheckBox.IsChecked = _settings.Enabled;
            RuleModeComboBox.SelectedValue = _settings.RuleMode;
            KeyboardEnterCheckBox.IsChecked = _settings.InterceptKeyboardEnter;
            NumpadCheckBox.IsChecked = _settings.InterceptNumpadEnter;
            ConfirmationModeComboBox.SelectedValue = _settings.Confirmation.Mode;
            HoldMillisecondsTextBox.Text = _settings.Confirmation.HoldMilliseconds.ToString(CultureInfo.InvariantCulture);
            ConfirmationPhraseTextBox.Text = _settings.Confirmation.Phrase;
            TimeoutSecondsTextBox.Text = _settings.Confirmation.TimeoutSeconds.ToString(CultureInfo.InvariantCulture);
            UnknownContextComboBox.SelectedValue = _settings.UnknownContextBehavior;
            StartupCheckBox.IsChecked = _settings.StartWithWindows;
            LogRetentionDaysTextBox.Text = _settings.LogRetentionDays.ToString(CultureInfo.InvariantCulture);
            RefreshActiveChatList();
            UpdateNumpadAvailability();
            SetNumericValidity(true, true, true);
            UpdateContextStatus(_services.ContextMonitor.Current);
            SetAutoSaveStatus("所有更改自动生效", isError: false);
        }
        finally
        {
            _loadingSettings = false;
        }
    }

    private async void AddCurrentChatButton_Click(object sender, RoutedEventArgs e)
    {
        await AddCurrentChatAsync();
    }

    private async Task AddCurrentChatAsync()
    {
        if (!await CommitSettingsAsync(allowInvalidNumbers: false))
        {
            return;
        }

        var context = _services.ContextMonitor.LastRecognizedWeixin;
        if (!context.IsTrustedWeixin || !context.IsCompatibilityAvailable || !context.IsKnownChat || context.TargetKind is null || string.IsNullOrWhiteSpace(context.ChatTitle))
        {
            MessageBox.Show(this, "请先在微信中打开一个可识别的群聊或联系人会话，并将光标放入消息输入框。", "无法加入", MessageBoxButton.OK, MessageBoxImage.Information);
            return;
        }

        var normalized = ChatTitleNormalizer.Normalize(context.ChatTitle);
        var exemptionList = IsExemptionMode;
        var currentList = exemptionList ? _services.Settings.ExemptedChats : _services.Settings.ProtectedChats;
        if (currentList.Any(chat => chat.TargetKind == context.TargetKind && ProtectedChatMatcher.TitleMatches(chat, normalized)))
        {
            MessageBox.Show(this, "这个会话已经在当前名单中。", "无需重复添加", MessageBoxButton.OK, MessageBoxImage.Information);
            return;
        }

        var chat = new ProtectedChat { DisplayName = normalized, MatchTitle = normalized, TargetKind = context.TargetKind.Value };
        if (await _services.AddChatAsync(chat, exemptionList))
        {
            _settings = _services.Settings;
            RefreshActiveChatList(chat.Id);
            StatusText.Text = exemptionList ? $"已加入免确认名单并生效：{normalized}" : $"已加入保护名单并生效：{normalized}";
            SetAutoSaveStatus("已自动保存", isError: false);
        }
    }

    private async void RemoveCurrentChatButton_Click(object sender, RoutedEventArgs e)
    {
        if (ActiveChatsList.SelectedItem is not ProtectedChat selected || !await CommitSettingsAsync(allowInvalidNumbers: false))
        {
            return;
        }

        await _services.RemoveChatAsync(selected.Id, IsExemptionMode);
        _settings = _services.Settings;
        RefreshActiveChatList();
        StatusText.Text = IsExemptionMode ? "已从免确认名单移除并立即生效" : "已从保护名单移除并立即生效";
        SetAutoSaveStatus("已自动保存", isError: false);
    }

    private async void ImportCurrentListButton_Click(object sender, RoutedEventArgs e)
    {
        if (!await CommitSettingsAsync(allowInvalidNumbers: false))
        {
            return;
        }

        var dialog = new OpenFileDialog { Filter = "会话配置 (*.json)|*.json|所有文件 (*.*)|*.*" };
        if (dialog.ShowDialog(this) != true)
        {
            return;
        }

        try
        {
            var chats = ProtectedChatExportCodec.Import(await File.ReadAllTextAsync(dialog.FileName));
            var next = IsExemptionMode
                ? _services.Settings with { ExemptedChats = chats.ToList() }
                : _services.Settings with { ProtectedChats = chats.ToList() };
            await _services.ApplySettingsAsync(next);
            _settings = _services.Settings;
            RefreshActiveChatList();
            StatusText.Text = IsExemptionMode ? "免确认名单已导入并生效" : "保护名单已导入并生效";
            SetAutoSaveStatus("已自动保存", isError: false);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or System.Text.Json.JsonException)
        {
            MessageBox.Show(this, $"导入失败：{ex.Message}", "导入会话配置", MessageBoxButton.OK, MessageBoxImage.Error);
        }
    }

    private async void ExportCurrentListButton_Click(object sender, RoutedEventArgs e)
    {
        if (!await CommitSettingsAsync(allowInvalidNumbers: false))
        {
            return;
        }

        var dialog = new SaveFileDialog
        {
            Filter = "会话配置 (*.json)|*.json",
            FileName = IsExemptionMode ? "exempted-chats.json" : "protected-chats.json",
        };
        if (dialog.ShowDialog(this) != true)
        {
            return;
        }

        try
        {
            var chats = IsExemptionMode ? _settings.ExemptedChats : _settings.ProtectedChats;
            await File.WriteAllTextAsync(dialog.FileName, ProtectedChatExportCodec.Export(chats));
            StatusText.Text = IsExemptionMode ? "免确认名单已导出" : "保护名单已导出";
        }
        catch (IOException ex)
        {
            MessageBox.Show(this, $"导出失败：{ex.Message}", "导出会话配置", MessageBoxButton.OK, MessageBoxImage.Error);
        }
    }

    private async void RuleModeComboBox_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingSettings || RuleModeComboBox.SelectedItem is not Choice<RuleMode>)
        {
            return;
        }

        _autoSaveTimer.Stop();
        if (await CommitSettingsAsync(allowInvalidNumbers: true))
        {
            RefreshActiveChatList();
            StatusText.Text = IsExemptionMode ? "已切换为免确认名单模式并生效" : "已切换为保护名单模式并生效";
        }
    }

    private async void ImmediateSettingChanged(object sender, RoutedEventArgs e)
    {
        if (_loadingSettings)
        {
            return;
        }

        _autoSaveTimer.Stop();
        UpdateNumpadAvailability();
        await CommitSettingsAsync(allowInvalidNumbers: true);
    }

    private async void ImmediateSettingChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingSettings)
        {
            return;
        }

        _autoSaveTimer.Stop();
        await CommitSettingsAsync(allowInvalidNumbers: true);
    }

    private void TextSettingChanged(object sender, TextChangedEventArgs e)
    {
        if (_loadingSettings)
        {
            return;
        }

        _autoSaveTimer.Stop();
        _autoSaveTimer.Start();
    }

    private async void TextSettingLostFocus(object sender, RoutedEventArgs e)
    {
        if (_loadingSettings)
        {
            return;
        }

        _autoSaveTimer.Stop();
        await CommitSettingsAsync(allowInvalidNumbers: false);
    }

    private async void AutoSaveTimer_Tick(object? sender, EventArgs e)
    {
        _autoSaveTimer.Stop();
        await CommitSettingsAsync(allowInvalidNumbers: false);
    }

    private async Task<bool> CommitSettingsAsync(bool allowInvalidNumbers)
    {
        if (_loadingSettings || !TryCreateSettings(allowInvalidNumbers, out var settings, out var validationMessage))
        {
            return false;
        }

        _settings = settings;
        try
        {
            await _services.ApplySettingsAsync(settings);
            _settings = _services.Settings;
            UpdateNumpadAvailability();
            SetAutoSaveStatus(validationMessage ?? "已自动保存", validationMessage is not null);
            return true;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or InvalidOperationException)
        {
            _settings = _services.Settings;
            SetAutoSaveStatus("未保存，仍使用上次有效设置", isError: true);
            return false;
        }
    }

    private bool TryCreateSettings(bool allowInvalidNumbers, out AppSettings settings, out string? validationMessage)
    {
        settings = _settings;
        validationMessage = null;
        if (ConfirmationModeComboBox.SelectedItem is not Choice<ConfirmationMode> confirmationMode
            || UnknownContextComboBox.SelectedItem is not Choice<UnknownContextBehavior> unknownBehavior
            || RuleModeComboBox.SelectedItem is not Choice<RuleMode> ruleMode)
        {
            return false;
        }

        var holdMilliseconds = ReadBoundedInteger(
            HoldMillisecondsTextBox.Text,
            _settings.Confirmation.HoldMilliseconds,
            500,
            3000,
            out var holdValid);
        var timeoutSeconds = ReadBoundedInteger(
            TimeoutSecondsTextBox.Text,
            _settings.Confirmation.TimeoutSeconds,
            5,
            30,
            out var timeoutValid);
        var logRetentionDays = ReadBoundedInteger(
            LogRetentionDaysTextBox.Text,
            _settings.LogRetentionDays,
            1,
            30,
            out var logRetentionValid);
        SetNumericValidity(holdValid, timeoutValid, logRetentionValid);

        if (!holdValid || !timeoutValid || !logRetentionValid)
        {
            validationMessage = "数字设置未应用，请修正红色输入框";
            if (!allowInvalidNumbers)
            {
                SetAutoSaveStatus(validationMessage, isError: true);
                return false;
            }
        }

        var protectedChats = _settings.ProtectedChats.ToList();
        var exemptedChats = _settings.ExemptedChats.ToList();
        if (_settings.RuleMode == RuleMode.ConfirmUnlessExcluded)
        {
            exemptedChats = UpdateSelectedAliases(exemptedChats, ActiveChatsList.SelectedItem);
        }
        else
        {
            protectedChats = UpdateSelectedAliases(protectedChats, ActiveChatsList.SelectedItem);
        }

        var keyboardEnter = KeyboardEnterCheckBox.IsChecked == true;
        settings = SettingsValidator.Sanitize(_settings with
        {
            Enabled = EnabledCheckBox.IsChecked == true,
            RuleMode = ruleMode.Value,
            Confirmation = _settings.Confirmation with
            {
                Mode = confirmationMode.Value,
                HoldMilliseconds = holdMilliseconds,
                Phrase = ConfirmationPhraseTextBox.Text,
                TimeoutSeconds = timeoutSeconds,
            },
            UnknownContextBehavior = unknownBehavior.Value,
            InterceptKeyboardEnter = keyboardEnter,
            InterceptNumpadEnter = keyboardEnter && NumpadCheckBox.IsChecked == true,
            ShiftEnterPassThrough = true,
            StartWithWindows = StartupCheckBox.IsChecked == true,
            LogRetentionDays = logRetentionDays,
            ProtectedChats = protectedChats,
            ExemptedChats = exemptedChats,
        });
        return true;
    }

    private void ActiveChatsList_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        ShowSelectedAliases(ActiveChatsList.SelectedItem as ProtectedChat);
    }

    private void RefreshActiveChatList(Guid? selectedId = null)
    {
        var wasLoading = _loadingSettings;
        _loadingSettings = true;
        _autoSaveTimer.Stop();
        try
        {
            var chats = IsExemptionMode ? _settings.ExemptedChats : _settings.ProtectedChats;
            ActiveChatListGroup.Header = IsExemptionMode ? "免确认的会话" : "需要二次确认的会话";
            ActiveListDescription.Text = IsExemptionMode
                ? "名单内的群聊或联系人可直接发送；其他已识别微信会话需要确认。"
                : "名单内的群聊或联系人需要二次确认；其他会话正常发送。";
            ActiveChatsList.ItemsSource = chats.ToList();
            if (selectedId is not null)
            {
                ActiveChatsList.SelectedItem = chats.FirstOrDefault(chat => chat.Id == selectedId.Value);
            }
        }
        finally
        {
            _loadingSettings = wasLoading;
        }

        ShowSelectedAliases(ActiveChatsList.SelectedItem as ProtectedChat);
    }

    private void ShowSelectedAliases(ProtectedChat? selected)
    {
        var wasLoading = _loadingSettings;
        _loadingSettings = true;
        _autoSaveTimer.Stop();
        try
        {
            AliasesTextBox.Text = selected is null
                ? string.Empty
                : string.Join(Environment.NewLine, selected.Aliases);
        }
        finally
        {
            _loadingSettings = wasLoading;
        }
    }

    private List<ProtectedChat> UpdateSelectedAliases(IEnumerable<ProtectedChat> chats, object? selectedItem)
    {
        if (selectedItem is not ProtectedChat selected)
        {
            return chats.ToList();
        }

        var aliases = AliasesTextBox.Text
            .Split(["\r\n", "\n"], StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .ToList();
        var updated = SettingsValidator.SanitizeChat(selected with { Aliases = aliases });
        return chats.Select(chat => chat.Id == selected.Id ? updated : chat).ToList();
    }

    private void UpdateNumpadAvailability()
    {
        NumpadCheckBox.IsEnabled = KeyboardEnterCheckBox.IsChecked == true;
        NumpadCheckBox.ToolTip = NumpadCheckBox.IsEnabled ? null : "启用主键盘 Enter 拦截后可用";
    }

    private static int ReadBoundedInteger(string text, int fallback, int minimum, int maximum, out bool valid)
    {
        valid = int.TryParse(text, NumberStyles.Integer, CultureInfo.InvariantCulture, out var value)
            && value >= minimum
            && value <= maximum;
        return valid ? value : fallback;
    }

    private void SetNumericValidity(bool holdValid, bool timeoutValid, bool logRetentionValid)
    {
        SetTextBoxValidity(HoldMillisecondsTextBox, holdValid);
        SetTextBoxValidity(TimeoutSecondsTextBox, timeoutValid);
        SetTextBoxValidity(LogRetentionDaysTextBox, logRetentionValid);
    }

    private static void SetTextBoxValidity(TextBox textBox, bool valid)
    {
        if (valid)
        {
            textBox.ClearValue(Control.BorderBrushProperty);
        }
        else
        {
            textBox.BorderBrush = Brushes.IndianRed;
        }
    }

    private void SetAutoSaveStatus(string text, bool isError)
    {
        AutoSaveStatusText.Text = text;
        AutoSaveStatusText.Foreground = isError ? Brushes.IndianRed : new SolidColorBrush(Color.FromRgb(22, 119, 255));
    }

    private void ContextMonitor_ContextChanged(object? sender, ChatContext context)
    {
        Dispatcher.InvokeAsync(() => UpdateContextStatus(context));
    }

    private void UpdateContextStatus(ChatContext context)
    {
        if (!context.IsTrustedWeixin)
        {
            StatusText.Text = context.RequiresElevation ? "微信权限不兼容" : "微信未在前台";
            CompatibilityHint.Text = context.RequiresElevation
                ? "微信以管理员权限运行。请用普通权限重新启动微信，本工具不会自动提权。"
                : "打开微信并将光标放入群聊或联系人输入框后，可加入当前名单。";
            return;
        }

        if (!context.IsCompatibilityAvailable)
        {
            StatusText.Text = "微信界面不可识别";
            CompatibilityHint.Text = "当前微信控件不可访问，工具不会宣称保护有效。";
            return;
        }

        var kind = context.TargetKind switch
        {
            ChatTargetKind.Group => "群聊",
            ChatTargetKind.Contact => "联系人",
            _ => "会话",
        };
        StatusText.Text = context.IsMessageEditorFocused
            ? $"当前{kind}：{context.ChatTitle ?? "未识别"}"
            : "微信已在前台，输入框未聚焦";
        CompatibilityHint.Text = IsExemptionMode
            ? "免确认名单内的群聊和联系人直接发送；其他微信会话需要确认。"
            : "保护名单内的群聊和联系人需要确认；其他会话正常发送。";
    }

    private sealed record Choice<T>(T Value, string Label)
    {
        public override string ToString() => Label;
    }
}
