# WeChatSendGuard UI & UX Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 重构 WeChatSendGuard 的设置界面（MainWindow）与二次确认弹窗（ConfirmationWindow），全面升级为直觉驱动的微信亲和绿色系（方案 A）现代视觉与交互体系，保持 800ms 长按确认机制。

**Architecture:** 
1. `DESIGN.md` 与 `App.xaml`：定义完整的微信亲和绿设计令牌（Design Tokens）与基础控件样式（按钮、卡片、徽章、输入框、列表项）。
2. `ConfirmationWindow`：实现浅绿目标大卡片、仿微信气泡草稿预览、内置 800ms 进度平滑填充按钮、默认聚焦取消按钮防手滑。
3. `MainWindow`：实现三层模块化架构（顶部状态药丸 + 左侧会话卡片与内联别名 + 右侧三张规则卡片）。
4. 保持 `WeChatSendGuard.Core` 业务模型与底层拦截逻辑 100% 兼容。

**Tech Stack:** C# 12 / .NET 9 / WPF (XAML)

---

### Task 1: 更新设计规范与全局样式系统 (App.xaml & DESIGN.md)

**Files:**
- Modify: `DESIGN.md`
- Modify: `src/WeChatSendGuard.App/App.xaml`

- [ ] **Step 1: 更新 `DESIGN.md`**
更新设计系统说明，反映方案 A 微信亲和绿色彩表、卡片圆角规则与直觉驱动交互准则。

- [ ] **Step 2: 更新 `App.xaml` 全局样式资源**
定义新的画刷资源与控件模板：
- 基础颜色画刷：`WindowBackgroundBrush` (`#F6F8FA`)、`PanelBrush` (`#FFFFFF`)、`PanelAltBrush` (`#F3F4F6`)、`AccentBrush` (`#07C160`)、`AccentHoverBrush` (`#06AD56`)、`AccentPressedBrush` (`#059B4C`)、`AccentSoftBrush` (`#E8F8F0`)、`InkBrush` (`#111827`)、`MutedBrush` (`#6B7280`)、`BorderBrush` (`#E2E8F0`)、`WarningBrush` (`#F59E0B`)、`DangerBrush` (`#EF4444`)。
- 徽章颜色：`BadgeGroupBgBrush` (`#E0F2FE`)、`BadgeGroupFgBrush` (`#0369A1`)、`BadgeContactBgBrush` (`#DCFCE7`)、`BadgeContactFgBrush` (`#15803D`)。
- 控件样式：PrimaryButtonStyle（绿色高亮）、SecondaryButtonStyle、DangerButtonStyle、CardBorderStyle、ModernTextBoxStyle、ModernComboBoxStyle、ModernListBoxItemStyle 等。

- [ ] **Step 3: 运行编译验证**
运行: `dotnet build src/WeChatSendGuard.App/WeChatSendGuard.App.csproj`
预期: Build succeeded.

---

### Task 2: 重构二次确认弹窗 (ConfirmationWindow.xaml & ConfirmationWindow.xaml.cs)

**Files:**
- Modify: `src/WeChatSendGuard.App/Windows/ConfirmationWindow.xaml`
- Modify: `src/WeChatSendGuard.App/Windows/ConfirmationWindow.xaml.cs`

- [ ] **Step 1: 重构 `ConfirmationWindow.xaml`**
- 纯白卡片容器 + 12px 圆角，宽度 480px，去除生冷灰底；
- Header：显示 `🛡️ 发送确认` 与右侧快捷键提示 `[Esc 取消]`；
- 目标卡片：浅绿底色 (`#E8F8F0`)，左侧展示会话类型徽章（`[群聊]` / `[联系人]`），右侧大号粗体标题；
- 草稿卡片：仿微信气泡样式背景，内容可折叠/滚动；
- 确认与取消区域：
  - 确认按钮（Hold 模式）：内置 800ms 平滑进度条，按住时平滑填充绿色；
  - 确认词模式：占位符高亮提示，回车即确认；
  - 取消按钮：默认初始键盘焦点，防止回车误触；
- 底部倒计时：细线进度条 + 动态秒数。

- [ ] **Step 2: 完善 `ConfirmationWindow.xaml.cs` 交互逻辑**
- 确保长按 800ms 计时器精准平滑更新；
- 确保鼠标松开或移出按钮时，进度立即重置归零；
- 默认在 `Loaded` 时将焦点赋予 `CancelButton`；
- 完善倒计时与 Esc 快捷取消。

- [ ] **Step 3: 运行编译验证**
运行: `dotnet build src/WeChatSendGuard.App/WeChatSendGuard.App.csproj`
预期: Build succeeded.

---

### Task 3: 重构主设置窗口 (MainWindow.xaml & MainWindow.xaml.cs)

**Files:**
- Modify: `src/WeChatSendGuard.App/MainWindow.xaml`
- Modify: `src/WeChatSendGuard.App/MainWindow.xaml.cs`

- [ ] **Step 1: 重构 `MainWindow.xaml` 布局**
- 顶部 Hero 栏：
  - 左侧：标题 `WeChatSendGuard` + 微信守护副标题；
  - 右侧：实时连接状态药丸（🟢 `守护中 · [群聊] 研发核心群` / ⚪ `未连接`）；
  - 中间/上方：大开关 `[✓] 启用发送保护` + 模式下拉框；
- 双栏工作区：
  - 左栏（46% 宽）：
    - 列表项使用 DataTemplate，包含 `[群聊]` / `[联系人]` 徽章、粗体名称、副标题别名展示；
    - 列表正下方紧邻内联别名编辑区；
    - 底部工具栏：`➕ 加入当前微信会话`（主按钮）、`🗑️ 移除`、`📥 导入`、`📤 导出`；
  - 右栏（54% 宽）：拆分为三张清晰卡片
    - 卡片 1【拦截按键】：主键盘 Enter、小键盘 Enter、Shift+Enter 提示；
    - 卡片 2【二次确认方式】：长按 800ms、单击、输入确认词；超时秒数；
    - 卡片 3【系统策略】：未识别会话行为、开机自启、日志天数；
- 底部状态栏：保存状态与醒目的 `[ 保存设置 ]` 绿色主按钮。

- [ ] **Step 2: 调整 `MainWindow.xaml.cs` 绑定与事件处理**
- 配合 DataTemplate 呈现群聊/联系人类型转换器或辅助属性；
- 优化选定项别名的即时双向同步；
- 保证微信前台状态检测的平滑更新。

- [ ] **Step 3: 运行编译与测试验证**
运行: `dotnet build` 与 `dotnet test`
预期: 全部 Build & Test 通过。

---

### Task 4: 整体回归与视觉验证

- [ ] **Step 1: 运行全量单元测试**
运行: `dotnet test`
预期: 所有的 Core 逻辑与测试通过。

- [ ] **Step 2: 验证 UI 交互与 XAML 规范**
确认所有样式引用无缺失、高 DPI 缩放与排版清晰无错位。
