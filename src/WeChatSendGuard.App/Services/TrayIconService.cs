using Drawing = System.Drawing;
using Forms = System.Windows.Forms;

namespace WeChatSendGuard.App.Services;

internal sealed class TrayIconService : IDisposable
{
    private readonly Forms.NotifyIcon _icon;
    private readonly Forms.ToolStripMenuItem _statusItem;
    private readonly Forms.ToolStripMenuItem _enableProtectionItem;
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
        
        // Status header (Read-only, clearly visible)
        _statusItem = new Forms.ToolStripMenuItem("正在检测微信...")
        {
            Enabled = false,
            Font = new Drawing.Font("Microsoft YaHei UI", 9F, Drawing.FontStyle.Regular)
        };

        // Open Settings (Primary Action)
        var openSettingsItem = new Forms.ToolStripMenuItem("打开主设置", null, (_, _) => showSettings())
        {
            Font = new Drawing.Font("Microsoft YaHei UI", 9F, Drawing.FontStyle.Bold)
        };

        // Protection Toggle (Stable text with clear Checkbox)
        _enableProtectionItem = new Forms.ToolStripMenuItem("启用发送守护")
        {
            CheckOnClick = true,
            Checked = true
        };
        _enableProtectionItem.CheckedChanged += (sender, _) =>
        {
            if (!_suppressToggle && sender is Forms.ToolStripMenuItem item)
            {
                _setEnabled(item.Checked);
            }
        };

        // Bypass Submenu
        var bypassMenu = new Forms.ToolStripMenuItem("临时放行当前会话");
        foreach (var minutes in new[] { 1, 5, 15 })
        {
            var item = new Forms.ToolStripMenuItem($"临时放行 {minutes} 分钟");
            item.Click += (_, _) => grantTemporaryBypass(minutes);
            bypassMenu.DropDownItems.Add(item);
        }

        var menu = new Forms.ContextMenuStrip();
        menu.Items.Add(_statusItem);
        menu.Items.Add(new Forms.ToolStripSeparator());
        menu.Items.Add(openSettingsItem);
        menu.Items.Add("加入当前微信会话", null, (_, _) => protectCurrentGroup());
        menu.Items.Add(bypassMenu);
        menu.Items.Add(new Forms.ToolStripSeparator());
        menu.Items.Add(_enableProtectionItem);
        menu.Items.Add(new Forms.ToolStripSeparator());
        menu.Items.Add("退出 WeChatSendGuard", null, (_, _) => exit());

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
        _enableProtectionItem.Checked = enabled;
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
