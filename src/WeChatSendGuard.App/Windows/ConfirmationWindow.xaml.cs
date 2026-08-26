using System.Windows;
using System.Windows.Input;
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
    private readonly DateTimeOffset _expiresAt;
    private bool _holding;
    private bool _completed;
    private DateTimeOffset _holdStartedAt;

    public ConfirmationWindow(PendingConfirmation pending, ConfirmationSettings settings)
    {
        InitializeComponent();
        _settings = settings;
        _expiresAt = pending.ExpiresAt;
        var targetName = pending.Decision.Kind switch
        {
            ProtectionDecisionKind.ConfirmProtected => pending.Decision.ProtectedChat?.DisplayName ?? pending.OriginalContext.ChatTitle ?? "当前会话",
            ProtectionDecisionKind.ConfirmUnlisted => pending.OriginalContext.ChatTitle ?? "当前会话",
            _ => "无法验证当前会话",
        };
        var targetKind = pending.OriginalContext.TargetKind switch
        {
            ChatTargetKind.Group => "群聊",
            ChatTargetKind.Contact => "联系人",
            _ => "会话",
        };
        TargetText.Text = pending.Decision.Kind == ProtectionDecisionKind.ConfirmUnknown
            ? targetName
            : $"{targetKind} · {targetName}";
        if (!string.IsNullOrWhiteSpace(pending.DraftPreview))
        {
            DraftPreviewText.Text = pending.DraftPreview;
            DraftPreviewPanel.Visibility = Visibility.Visible;
        }

        _timeoutTimer = new DispatcherTimer(DispatcherPriority.Input) { Interval = TimeSpan.FromMilliseconds(100) };
        _timeoutTimer.Tick += TimeoutTimer_Tick;
        _holdTimer = new DispatcherTimer(DispatcherPriority.Input) { Interval = TimeSpan.FromMilliseconds(25) };
        _holdTimer.Tick += HoldTimer_Tick;
        Closed += ConfirmationWindow_Closed;

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
                InstructionText.Text = "请确认上方显示的聊天名称，然后点击确认。";
                break;
            case ConfirmationMode.Hold:
                InstructionText.Text = $"请按住“确认发送” {_settings.HoldMilliseconds} 毫秒。";
                HoldProgress.Visibility = Visibility.Visible;
                ConfirmButton.Content = "按住确认发送";
                ConfirmButton.PreviewMouseLeftButtonDown += ConfirmButton_PreviewMouseLeftButtonDown;
                ConfirmButton.PreviewMouseLeftButtonUp += ConfirmButton_PreviewMouseLeftButtonUp;
                ConfirmButton.PreviewMouseMove += ConfirmButton_PreviewMouseMove;
                ConfirmButton.MouseLeave += ConfirmButton_MouseLeave;
                ConfirmButton.LostMouseCapture += ConfirmButton_LostMouseCapture;
                break;
            case ConfirmationMode.Phrase:
                InstructionText.Text = $"请输入“{_settings.Phrase}”后确认。";
                PhraseBox.Visibility = Visibility.Visible;
                PhraseBox.TextChanged += PhraseBox_TextChanged;
                PhraseBox.KeyDown += PhraseBox_KeyDown;
                ConfirmButton.IsEnabled = false;
                break;
        }
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
        _holdTimer.Start();
    }

    private void ConfirmButton_PreviewMouseLeftButtonUp(object sender, MouseButtonEventArgs e)
    {
        // A user can release between timer ticks just after reaching the
        // threshold. Treat that as a completed hold instead of dropping it.
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
        if (position.X < 0 || position.Y < 0 || position.X > ConfirmButton.ActualWidth || position.Y > ConfirmButton.ActualHeight)
        {
            StopHolding();
        }
    }

    private void ConfirmButton_MouseLeave(object sender, System.Windows.Input.MouseEventArgs e)
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
        HoldProgress.Value = Math.Clamp(elapsed.TotalMilliseconds / _settings.HoldMilliseconds * 100, 0, 100);
        if (elapsed.TotalMilliseconds >= _settings.HoldMilliseconds)
        {
            Complete(ConfirmationOutcome.Confirmed);
        }
    }

    private void StopHolding()
    {
        _holding = false;
        _holdTimer.Stop();
        if (Mouse.Captured == ConfirmButton)
        {
            Mouse.Capture(null);
        }

        HoldProgress.Value = 0;
    }

    private void PhraseBox_TextChanged(object sender, System.Windows.Controls.TextChangedEventArgs e)
    {
        ConfirmButton.IsEnabled = string.Equals(PhraseBox.Text.Trim(), _settings.Phrase, StringComparison.Ordinal);
    }

    private void PhraseBox_KeyDown(object sender, System.Windows.Input.KeyEventArgs e)
    {
        if (e.Key == Key.Enter && ConfirmButton.IsEnabled)
        {
            Complete(ConfirmationOutcome.Confirmed);
            e.Handled = true;
        }
    }

    private void TimeoutTimer_Tick(object? sender, EventArgs e)
    {
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
        CountdownText.Text = $"将在 {Math.Max(0, remaining.TotalSeconds):0.0} 秒后自动取消";
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
