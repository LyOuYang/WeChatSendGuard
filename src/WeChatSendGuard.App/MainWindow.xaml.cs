using System.Globalization;
using System.Text;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using Microsoft.Win32;
using WeChatSendGuard.App.Services;
using WeChatSendGuard.Core.Configuration;
using WeChatSendGuard.Core.Guard;

namespace WeChatSendGuard.App;

public partial class MainWindow : Window
{
    private readonly AppServices _services;
    private AppSettings _settings;
    private bool _loadingSettings;

    internal MainWindow(AppServices services, AppSettings settings)
    {
        _services = services;
        _settings = SettingsValidator.Sanitize(settings);
        _loadingSettings = true;

        InitializeComponent();
        ConfigureChoices();
        LoadSettings(_settings);
        _services.ContextMonitor.ContextChanged += ContextMonitor_ContextChanged;
        Closed += (_, _) =>
        {
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
            SetSaveStatus("确认设置后点击保存", isError: false);
        }
        finally
        {
            _loadingSettings = false;
        }
    }

    private async void SaveSettingsButton_Click(object sender, RoutedEventArgs e)
    {
        await CommitSettingsAsync();
    }

    private async void AddCurrentChatButton_Click(object sender, RoutedEventArgs e)
    {
        await AddCurrentChatAsync();
    }

    private async Task AddCurrentChatAsync()
    {
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
            SetSaveStatus("会话名单已立即生效", isError: false);
        }
    }

    private async void RemoveCurrentChatButton_Click(object sender, RoutedEventArgs e)
    {
        var selectedIds = ActiveChatsList.SelectedItems
            .OfType<ProtectedChat>()
            .Select(static chat => chat.Id)
            .ToList();
        if (selectedIds.Count == 0)
        {
            return;
        }

        await _services.RemoveChatsAsync(selectedIds, IsExemptionMode);
        _settings = _services.Settings;
        RefreshActiveChatList();
        StatusText.Text = IsExemptionMode ? $"已从免确认名单移除 {selectedIds.Count} 项并生效" : $"已从保护名单移除 {selectedIds.Count} 项并生效";
        SetSaveStatus("会话名单已立即生效", isError: false);
    }

    private async void ImportCurrentListButton_Click(object sender, RoutedEventArgs e)
    {
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
            SetSaveStatus("会话名单已立即生效", isError: false);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or System.Text.Json.JsonException)
        {
            MessageBox.Show(this, $"导入失败：{ex.Message}", "导入会话配置", MessageBoxButton.OK, MessageBoxImage.Error);
        }
    }

    private async void ExportCurrentListButton_Click(object sender, RoutedEventArgs e)
    {
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
            var chats = IsExemptionMode ? _services.Settings.ExemptedChats : _services.Settings.ProtectedChats;
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
        if (_loadingSettings || RuleModeComboBox.SelectedItem is not Choice<RuleMode> selectedMode)
        {
            return;
        }

        try
        {
            await _services.ApplySettingsAsync(_services.Settings with { RuleMode = selectedMode.Value });
            _settings = _services.Settings;
            RefreshActiveChatList();
            StatusText.Text = IsExemptionMode ? "已切换为免确认名单模式并生效" : "已切换为保护名单模式并生效";
            SetSaveStatus("当前名单模式已立即生效", isError: false);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or InvalidOperationException)
        {
            _settings = _services.Settings;
            _loadingSettings = true;
            try
            {
                RuleModeComboBox.SelectedValue = _settings.RuleMode;
                RefreshActiveChatList();
            }
            finally
            {
                _loadingSettings = false;
            }

            SetSaveStatus("名单模式未保存，仍使用上次有效设置", isError: true);
        }
    }

    private void SettingChanged(object sender, RoutedEventArgs e)
    {
        if (_loadingSettings)
        {
            return;
        }

        UpdateNumpadAvailability();
        SetSaveStatus("更改尚未保存", isError: false);
    }

    private void SettingChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_loadingSettings)
        {
            return;
        }

        SetSaveStatus("更改尚未保存", isError: false);
    }

    private void TextSettingChanged(object sender, TextChangedEventArgs e)
    {
        if (_loadingSettings)
        {
            return;
        }

        SetSaveStatus("更改尚未保存", isError: false);
    }

    private async Task<bool> CommitSettingsAsync()
    {
        if (_loadingSettings || !TryCreateSettings(out var settings))
        {
            return false;
        }

        try
        {
            await _services.ApplySettingsAsync(settings);
            _settings = _services.Settings;
            SynchronizeNumericTextBoxes(_settings);
            UpdateNumpadAvailability();
            RefreshActiveChatList((ActiveChatsList.SelectedItem as ProtectedChat)?.Id);
            SetSaveStatus("设置已保存并生效", isError: false);
            return true;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or InvalidOperationException)
        {
            _settings = _services.Settings;
            SetSaveStatus("未保存，仍使用上次有效设置", isError: true);
            return false;
        }
    }

    private bool TryCreateSettings(out AppSettings settings)
    {
        settings = _settings;
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
            1,
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
            SetSaveStatus("请修正红色数字输入框后再保存", isError: true);
            return false;
        }

        var protectedChats = _settings.ProtectedChats.ToList();
        var exemptedChats = _settings.ExemptedChats.ToList();
        if (ruleMode.Value == RuleMode.ConfirmUnlessExcluded)
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
        ShowSelectedAliases();
    }

    private void RefreshActiveChatList(Guid? selectedId = null)
    {
        var wasLoading = _loadingSettings;
        _loadingSettings = true;
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

        ShowSelectedAliases();
    }

    private void ShowSelectedAliases()
    {
        var selectedChats = ActiveChatsList.SelectedItems.OfType<ProtectedChat>().Take(2).ToList();
        var selected = selectedChats.Count == 1 ? selectedChats[0] : null;
        var wasLoading = _loadingSettings;
        _loadingSettings = true;
        try
        {
            AliasesTextBox.IsEnabled = selected is not null;
            AliasesTextBox.ToolTip = selectedChats.Count switch
            {
                0 => "选择一个会话后可编辑别名",
                1 => null,
                _ => "同时选择多个会话时不能编辑别名",
            };
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
        if (ActiveChatsList.SelectedItems.Count != 1 || selectedItem is not ProtectedChat selected)
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
        var normalized = text.Normalize(NormalizationForm.FormKC).Trim();
        if (normalized.Length == 0)
        {
            valid = true;
            return fallback;
        }

        valid = int.TryParse(normalized, NumberStyles.Integer, CultureInfo.InvariantCulture, out var value)
            && value >= minimum
            && value <= maximum;
        return valid ? value : fallback;
    }

    private void SynchronizeNumericTextBoxes(AppSettings settings)
    {
        var wasLoading = _loadingSettings;
        _loadingSettings = true;
        try
        {
            HoldMillisecondsTextBox.Text = settings.Confirmation.HoldMilliseconds.ToString(CultureInfo.InvariantCulture);
            TimeoutSecondsTextBox.Text = settings.Confirmation.TimeoutSeconds.ToString(CultureInfo.InvariantCulture);
            LogRetentionDaysTextBox.Text = settings.LogRetentionDays.ToString(CultureInfo.InvariantCulture);
            SetNumericValidity(true, true, true);
        }
        finally
        {
            _loadingSettings = wasLoading;
        }
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

    private void SetSaveStatus(string text, bool isError)
    {
        SaveStatusText.Text = text;
        SaveStatusText.Foreground = isError ? Brushes.IndianRed : new SolidColorBrush(Color.FromRgb(22, 119, 255));
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
