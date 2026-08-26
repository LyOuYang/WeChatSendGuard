using Drawing = System.Drawing;
using Forms = System.Windows.Forms;

namespace WeChatSendGuard.App.Services;

internal sealed class TrayIconService : IDisposable
{
    private readonly Forms.NotifyIcon _icon;
    private readonly Forms.ToolStripMenuItem _statusItem;
    private readonly Forms.ToolStripMenuItem _pauseItem;
    private readonly Action<bool> _setEnabled;
    private bool _suppressToggle;
    private bool _disposed;

    public TrayIconService(
        Action showSettings,
        Action protectCurrentGroup,
        Action<int> grantTemporaryBypass,
        Action<bool> setEnabled,
        Action showStatus,
        Action exit)
    {
        _setEnabled = setEnabled;
        _statusItem = new Forms.ToolStripMenuItem("正在启动...") { Enabled = false };
        _pauseItem = new Forms.ToolStripMenuItem("启用发送保护") { CheckOnClick = true };
        _pauseItem.CheckedChanged += (_, _) =>
        {
            if (!_suppressToggle)
            {
                _setEnabled(_pauseItem.Checked);
            }
        };

        var bypassMenu = new Forms.ToolStripMenuItem("临时放行当前会话");
        foreach (var minutes in new[] { 1, 5, 15 })
        {
            var item = new Forms.ToolStripMenuItem($"放行 {minutes} 分钟");
            item.Click += (_, _) => grantTemporaryBypass(minutes);
            bypassMenu.DropDownItems.Add(item);
        }

        var menu = new Forms.ContextMenuStrip();
        menu.Items.Add(_statusItem);
        menu.Items.Add(new Forms.ToolStripSeparator());
        menu.Items.Add("打开设置", null, (_, _) => showSettings());
        menu.Items.Add("加入当前名单", null, (_, _) => protectCurrentGroup());
        menu.Items.Add(bypassMenu);
        menu.Items.Add(_pauseItem);
        menu.Items.Add("查看状态", null, (_, _) => showStatus());
        menu.Items.Add(new Forms.ToolStripSeparator());
        menu.Items.Add("退出", null, (_, _) => exit());

        _icon = new Forms.NotifyIcon
        {
            Icon = Drawing.SystemIcons.Shield,
            Visible = true,
            Text = "WeChatSendGuard",
            ContextMenuStrip = menu,
        };
        _icon.DoubleClick += (_, _) => showSettings();
    }

    public void SetProtectionEnabled(bool enabled)
    {
        _suppressToggle = true;
        _pauseItem.Checked = enabled;
        _pauseItem.Text = enabled ? "启用发送保护" : "发送保护已暂停";
        _suppressToggle = false;
    }

    public void SetStatus(string status)
    {
        _statusItem.Text = status;
        _icon.Text = status.Length > 60 ? status[..60] : status;
    }

    public void ShowInformation(string title, string message)
    {
        _icon.ShowBalloonTip(3000, title, message, Forms.ToolTipIcon.Info);
    }

    public void ShowWarning(string title, string message)
    {
        _icon.ShowBalloonTip(4000, title, message, Forms.ToolTipIcon.Warning);
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        _icon.Visible = false;
        _icon.Dispose();
    }
}
