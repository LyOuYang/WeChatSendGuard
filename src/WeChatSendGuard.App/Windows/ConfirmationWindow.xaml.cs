using System.Windows;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;
using WeChatSendGuard.Core.Configuration;
using WeChatSendGuard.Core.Guard;

namespace WeChatSendGuard.App.Windows;

public partial class ConfirmationWindow : Window
{
    private readonly ConfirmationSettings _settings;
    private readonly DispatcherTimer _timeoutTimer;
    private readonly DispatcherTimer _holdTimer;
    private readonly TaskCompletionSource<ConfirmationOutcome> _completion = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private DateTimeOffset _expiresAt;
    private readonly double _totalTimeoutSeconds;
    private bool _holding;
    private bool _completed;
    private DateTimeOffset _holdStartedAt;

    public ConfirmationWindow(PendingConfirmation pending, ConfirmationSettings settings)
    {
        InitializeComponent();
        _settings = settings;
        _expiresAt = pending.ExpiresAt;
        _totalTimeoutSeconds = Math.Max(1.0, (_expiresAt - DateTimeOffset.UtcNow).TotalSeconds);

        var targetName = pending.Decision.Kind switch
        {
            ProtectionDecisionKind.ConfirmProtected => pending.Decision.ProtectedChat?.DisplayName ?? pending.OriginalContext.ChatTitle ?? "当前会话",
            ProtectionDecisionKind.ConfirmUnlisted => pending.OriginalContext.ChatTitle ?? "当前会话",
            _ => "无法验证当前会话",
        };

        var isContact = pending.OriginalContext.TargetKind == ChatTargetKind.Contact;
        TargetKindText.Text = pending.OriginalContext.TargetKind switch
        {
            ChatTargetKind.Group => "群聊",
            ChatTargetKind.Contact => "联系人",
            _ => "会话",
        };

        if (isContact)
        {
            TargetKindBadge.Background = (Brush)FindResource("BadgeContactBgBrush");
            TargetKindText.Foreground = (Brush)FindResource("BadgeContactFgBrush");
        }
        else
        {
            TargetKindBadge.Background = (Brush)FindResource("BadgeGroupBgBrush");
            TargetKindText.Foreground = (Brush)FindResource("BadgeGroupFgBrush");
        }

        TargetText.Text = targetName;

        if (!string.IsNullOrWhiteSpace(pending.DraftPreview))
        {
            DraftPreviewText.Text = pending.DraftPreview;
            DraftPreviewPanel.Visibility = Visibility.Visible;
        }

        _timeoutTimer = new DispatcherTimer(DispatcherPriority.Input) { Interval = TimeSpan.FromMilliseconds(50) };
        _timeoutTimer.Tick += TimeoutTimer_Tick;
        _holdTimer = new DispatcherTimer(DispatcherPriority.Input) { Interval = TimeSpan.FromMilliseconds(20) };
        _holdTimer.Tick += HoldTimer_Tick;
        Closed += ConfirmationWindow_Closed;
        Loaded += ConfirmationWindow_Loaded;

        ConfigureMode();
        UpdateCountdown();
        _timeoutTimer.Start();
    }

    public Task<ConfirmationOutcome> Completion => _completion.Task;

    public void Cancel()
    {
        Complete(ConfirmationOutcome.Cancelled);
    }

    private void ConfigureMode()
    {
        switch (_settings.Mode)
        {
            case ConfirmationMode.Click:
                ConfirmButton.Content = "确认发送";
                break;

            case ConfirmationMode.Hold:
                var seconds = _settings.HoldMilliseconds / 1000.0;
                ConfirmButton.Content = $"按住确认 ({seconds:0.#}s)";
                ConfirmButton.PreviewMouseLeftButtonDown += ConfirmButton_PreviewMouseLeftButtonDown;
                ConfirmButton.PreviewMouseLeftButtonUp += ConfirmButton_PreviewMouseLeftButtonUp;
                ConfirmButton.PreviewMouseMove += ConfirmButton_PreviewMouseMove;
                ConfirmButton.MouseLeave += ConfirmButton_MouseLeave;
                ConfirmButton.LostMouseCapture += ConfirmButton_LostMouseCapture;
                break;

            case ConfirmationMode.Phrase:
                PhraseBox.Visibility = Visibility.Visible;
                PhraseBox.TextChanged += PhraseBox_TextChanged;
                PhraseBox.KeyDown += PhraseBox_KeyDown;
                ConfirmButton.IsEnabled = false;
                ConfirmButton.Content = "确认发送";
                break;
        }
    }

    private void ConfirmationWindow_Loaded(object sender, RoutedEventArgs e)
    {
        if (_settings.Mode == ConfirmationMode.Phrase)
        {
            PhraseBox.Focus();
            Keyboard.Focus(PhraseBox);
            return;
        }

        CancelButton.Focus();
        Keyboard.Focus(CancelButton);
    }

    private void ConfirmationWindow_KeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key != Key.Escape)
        {
            return;
        }

        Complete(ConfirmationOutcome.Cancelled);
        e.Handled = true;
    }

    private void ConfirmButton_Click(object sender, RoutedEventArgs e)
    {
        if (_settings.Mode != ConfirmationMode.Hold)
        {
            Complete(ConfirmationOutcome.Confirmed);
        }
    }

    private void CancelButton_Click(object sender, RoutedEventArgs e) => Complete(ConfirmationOutcome.Cancelled);

    private void ConfirmButton_PreviewMouseLeftButtonDown(object sender, MouseButtonEventArgs e)
    {
        if (_completed)
        {
            return;
        }

        _holding = true;
        _holdStartedAt = DateTimeOffset.UtcNow;
        ConfirmButton.CaptureMouse();
        
        // 核心交互：长按期间倒计时暂停
        _timeoutTimer.Stop();
        HoldProgress.Visibility = Visibility.Visible;
        _holdTimer.Start();
    }

    private void ConfirmButton_PreviewMouseLeftButtonUp(object sender, MouseButtonEventArgs e)
    {
        if (!_completed
            && _holding
            && DateTimeOffset.UtcNow - _holdStartedAt >= TimeSpan.FromMilliseconds(_settings.HoldMilliseconds))
        {
            Complete(ConfirmationOutcome.Confirmed);
            return;
        }

        StopHolding();
    }

    private void ConfirmButton_PreviewMouseMove(object sender, MouseEventArgs e)
    {
        if (!_holding)
        {
            return;
        }

        var position = e.GetPosition(ConfirmButton);
        if (position.X < -10 || position.Y < -10 || position.X > ConfirmButton.ActualWidth + 10 || position.Y > ConfirmButton.ActualHeight + 10)
        {
            StopHolding();
        }
    }

    private void ConfirmButton_MouseLeave(object sender, MouseEventArgs e)
    {
        if (_holding)
        {
            StopHolding();
        }
    }

    private void ConfirmButton_LostMouseCapture(object sender, MouseEventArgs e) => StopHolding();

    private void HoldTimer_Tick(object? sender, EventArgs e)
    {
        if (!_holding)
        {
            return;
        }

        var elapsed = DateTimeOffset.UtcNow - _holdStartedAt;
        var percent = Math.Clamp(elapsed.TotalMilliseconds / _settings.HoldMilliseconds * 100, 0, 100);
        HoldProgress.Value = percent;

        var remainingSeconds = Math.Max(0, (_settings.HoldMilliseconds - elapsed.TotalMilliseconds) / 1000.0);
        ConfirmButton.Content = remainingSeconds <= 0.05 ? "✓ 确认发送" : $"按住确认 ({remainingSeconds:0.1}s)";

        if (elapsed.TotalMilliseconds >= _settings.HoldMilliseconds)
        {
            Complete(ConfirmationOutcome.Confirmed);
        }
    }

    private void StopHolding()
    {
        if (!_holding)
        {
            return;
        }

        _holding = false;
        _holdTimer.Stop();
        if (Mouse.Captured == ConfirmButton)
        {
            Mouse.Capture(null);
        }

        HoldProgress.Value = 0;
        HoldProgress.Visibility = Visibility.Collapsed;

        if (_settings.Mode == ConfirmationMode.Hold)
        {
            var seconds = _settings.HoldMilliseconds / 1000.0;
            ConfirmButton.Content = $"按住确认 ({seconds:0.#}s)";
        }

        // 核心交互：松手未完成长按时，超时倒计时顺延并恢复继续递减
        if (!_completed)
        {
            var heldDuration = DateTimeOffset.UtcNow - _holdStartedAt;
            _expiresAt = _expiresAt.Add(heldDuration);
            UpdateCountdown();
            _timeoutTimer.Start();
        }
    }

    private void PhraseBox_TextChanged(object sender, System.Windows.Controls.TextChangedEventArgs e)
    {
        ConfirmButton.IsEnabled = string.Equals(PhraseBox.Text.Trim(), _settings.Phrase, StringComparison.Ordinal);
    }

    private void PhraseBox_KeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Enter && ConfirmButton.IsEnabled)
        {
            Complete(ConfirmationOutcome.Confirmed);
            e.Handled = true;
        }
    }

    private void TimeoutTimer_Tick(object? sender, EventArgs e)
    {
        if (_holding)
        {
            return;
        }

        if (DateTimeOffset.UtcNow >= _expiresAt)
        {
            Complete(ConfirmationOutcome.TimedOut);
            return;
        }

        UpdateCountdown();
    }

    private void UpdateCountdown()
    {
        var remaining = _expiresAt - DateTimeOffset.UtcNow;
        var remainingSeconds = Math.Max(0, remaining.TotalSeconds);
        CountdownText.Text = $"{remainingSeconds:0.0} 秒后自动取消";
        CountdownProgress.Value = Math.Clamp((remainingSeconds / _totalTimeoutSeconds) * 100, 0, 100);
    }

    private void Complete(ConfirmationOutcome outcome)
    {
        if (_completed)
        {
            return;
        }

        _completed = true;
        StopHolding();
        _timeoutTimer.Stop();
        _completion.TrySetResult(outcome);
        Close();
    }

    private void ConfirmationWindow_Closed(object? sender, EventArgs e)
    {
        _timeoutTimer.Stop();
        _holdTimer.Stop();
        if (!_completed)
        {
            _completed = true;
            _completion.TrySetResult(ConfirmationOutcome.Cancelled);
        }
    }
}
