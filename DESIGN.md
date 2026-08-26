# Design System: WeChatSendGuard

## 1. 核心理念与风格定位

**直觉驱动 (Intuition-Driven) · 微信原生亲和风格 (WeChat Modern Green)**
- **场景**：桌面常驻守护工具。平时安静低打扰，拦截时 0.1 秒认知，确认时绝对防手滑。
- **原则**：减少长篇说教文字，依靠卡片分块、色彩徽章、自解释控件与微交互传递状态与操作路径。

---

## 2. 视觉色彩系统 (WeChat Green Palette)

| 资源键名 | 颜色值 (HEX) | 语义与用途 |
| :--- | :--- | :--- |
| `WindowBackgroundBrush` | `#F6F8FA` | 窗口底层背景，柔和护眼浅灰 |
| `PanelBrush` | `#FFFFFF` | 主卡片/容器表面底色（纯白 + 1px 细边框） |
| `PanelAltBrush` | `#F3F4F6` | 次级容器底色（输入框底色、草稿预览气泡） |
| `AccentBrush` | `#07C160` | 微信官方品牌绿（主操作按钮、长按进度高亮、开启状态） |
| `AccentHoverBrush` | `#06AD56` | 主色悬停微反馈 |
| `AccentPressedBrush` | `#059B4C` | 主色按下微反馈 |
| `AccentSoftBrush` | `#E8F8F0` | 浅绿背景（列表选中高亮、弹窗目标卡片底色） |
| `InkBrush` | `#111827` | 主标题与正文（高对比度 Slate 900 深黑） |
| `MutedBrush` | `#6B7280` | 次级辅助文字（Slate 500） |
| `BorderBrush` | `#E2E8F0` | 默认边框线条 |
| `BorderFocusBrush` | `#07C160` | 输入聚焦与选中边框高亮 |
| `WarningBrush` | `#F59E0B` | 倒计时与提醒琥珀黄 |
| `DangerBrush` | `#EF4444` | 危险、删除与拦截失败珊瑚红 |
| `BadgeGroupBgBrush` | `#E0F2FE` | 群聊标签背景（浅蓝） |
| `BadgeGroupFgBrush` | `#0369A1` | 群聊标签文字（深蓝） |
| `BadgeContactBgBrush`| `#DCFCE7` | 联系人标签背景（浅绿） |
| `BadgeContactFgBrush`| `#15803D` | 联系人标签文字（深绿） |

---

## 3. 排版与层次 (Typography & Hierarchy)

- **字体族**：`Segoe UI`, `Microsoft YaHei UI`, `PingFang SC`, sans-serif
- **层级规范**：
  - 窗口主标题：`20px` / `FontWeight: SemiBold` / `#111827`
  - 卡片分组标题：`14px` / `FontWeight: SemiBold` / `#111827`
  - 正文与列表标题：`13px` / `FontWeight: Normal` 或 `Medium`
  - 徽章与辅助说明：`11px ~ 12px` / `FontWeight: Medium`

---

## 4. 组件规范 (Components)

- **卡片容器 (Card Surface)**：纯白背景，`CornerRadius="8"`，`BorderThickness="1"`，边框颜色 `#E2E8F0`。
- **主按钮 (Primary Button)**：`Background="#07C160"`，前景纯白，圆角 `6px`，高度 `34px`，悬停 `#06AD56`。
- **次级按钮 (Secondary Button)**：`Background="#FFFFFF"`，边框 `#E2E8F0`，文字 `#111827`，悬停 `#F3F4F6`。
- **输入框 (TextBox & ComboBox)**：高度 `34px`，圆角 `6px`，聚焦时边框高亮为 `#07C160`。
- **长按确认按钮 (Hold Progress Button)**：内置平滑进度条填充，800ms 从左向右充满，松开立即重置。
- **徽章 (Badge)**：高度 `20px`，圆角 `4px`，用于标注群聊（蓝）和联系人（绿）。
