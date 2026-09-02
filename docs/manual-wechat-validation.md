# 人工微信验证清单

## 前置条件

- 使用专用测试账号和没有无关收件人的私有测试群或收藏/文件传输目标。
- 记录目标平台版本、微信版本、应用版本、安装包 SHA-256、测试日期和测试人；macOS 还需记录 CPU 架构、bundle/Team 签名身份与公证结果。
- 安装候选包后从全新应用启动开始验证。
- 验证期间不运行任何可能操作真实微信的自动化工具。

## 设置与托盘

1. 验证首次启动打开设置窗口，`--silent` 在托盘启动。
2. 在名单模式和白名单模式之间切换，验证二者保留独立名单。
3. 人工加入一个群聊和一个联系人，验证类型标签与别名。
4. 修改非名单设置后不保存就关闭并重开，验证旧生效值未变；保存后重启，验证新值已持久化。
5. 检查默认微信路径自动显示为 `C:\Program Files\Tencent\Weixin\Weixin.exe`。在可控测试环境中保存一个实际的其他本地 `Weixin.exe` 路径，再恢复默认，验证每次保存后状态重新观察且错误路径不会导致输入注入。
6. 分别从主开关和托盘切换守护，验证无需点击保存立即生效、重启后保持正确，托盘同时显示当前状态和下一步“关闭”或“启用”。
7. 从托盘分别选择暂停 1、5、15 分钟，验证键盘 Enter 和发送按钮在暂停期间直接保持微信行为，状态显示剩余时间，到期后自动恢复；暂停期间已有确认请求应被取消。
8. 最小化设置窗口后点击托盘图标，验证原窗口被恢复而不是继续最小化。
9. 从托盘设置临时放行，验证在非“仅保护指定名单”模式下不可用或被拒绝。

## 发送保护

1. 在受保护群聊编辑框中输入无害且不发送的草稿，按主键盘 Enter；验证草稿未发送、确认窗居中于微信窗口并尝试获得焦点。
2. 在启用数字键盘 Enter 时重复验证；关闭后按客户端正常行为确认其放行。
3. 验证 `Shift+Enter`、`Ctrl+Enter` 及其他带修饰键的 Enter 组合不会弹出守护确认窗，输入法候选确认也保持放行。
4. 在长按模式中分别提前松开、移出按钮、按 Esc、点击取消和等待超时；每种情况草稿均不得发送，Esc 必须取消确认窗而不能影响或最小化微信。
5. 完成长按，验证确认窗关闭、原编辑框仍是目标，并且只发送一条消息。
6. 确认窗显示时切换到其他聊天或其他窗口后再确认，验证绝不注入消息。
7. 验证单击和确认词模式，包括确认词不匹配和按 Enter 确认。
8. 验证未知或不兼容上下文遵从安全配置，身份无法重新校验时绝不注入。
9. 在相同私有测试会话中分别验证两栏与三栏布局：保持第三栏打开后，连续 20 次按 Enter、连续 20 次点击“发送”；每次都应弹出确认或在界面暂不可识别时安全阻止，不能直接发送。
10. 在三栏布局中反复打开/关闭右侧详情或网页栏并切换会话；确认普通输入、滚动和右侧栏操作没有明显卡顿，受保护会话的 Enter 与“发送”仍能稳定进入确认流程。
11. 每轮两栏/三栏测试前后记录时间，在 `%LocalAppData%\WeChatSendGuard\logs` 中截取对应时间段的日志：按钮测试应能看到 `send-button-diagnostic`，随后看到 `confirmation` 或 `send-blocked`；确认发送后应看到同一 `traceId` 的 `send`。若看到 `send-button-snapshot-stale`、`send-button-not-found` 或 `send-button-geometry-unavailable`，同时检查是否出现 `button-fallback-intercepted`，以区分安全兜底拦截和识别仍未命中。
12. 保持两栏和三栏各测试一次：拖动微信窗口但不改变大小后立即点击“发送”，应直接弹出确认；改变窗口大小、跨不同 DPI 显示器移动或打开/关闭第三栏后立即点击，允许短暂进入窄区兜底，但不能直接发送。关闭“拦截发送按钮”后，上述点击应完全按微信原生行为执行且日志不再生成按钮诊断；重新开启后无需重启即可恢复。

## Windows 不可识别诊断判读

1. 每条 schema v2 审计记录应依次以 `localTime`、`applicationVersion`、`weixinVersion` 开头，并包含 `sessionId` 和 `processId`；同一时间出现多个 session/process 表示存在多个守护实例。
2. `trustedWeixin=true` 且 `contextCompatibilityAvailable=false` 表示自定义路径信任已通过，失败发生在 UIA 界面识别，不应再归因于“没有找到 Weixin.exe”。
3. `uiaRootAvailable=false` 或 `uiaStatus=query-failed` 表示没有取得可用根节点；结合 `uiaErrorCode`（含可安全提取时的 HRESULT）检查 COM/UIA、权限或运行环境。
4. `uiaRootAvailable=true` 后先检查 `uiaTreeQueryStatus`：`query-failed`/`length-failed` 结合 `uiaTreeErrorCode` 表示整树枚举失败；`success` 但 `uiaTreeDescendantCount=0` 表示只取得原生窗口壳；`partial-property-read-failure` 表示子节点存在但控件类型、AutomationId 或 ClassName 至少有一类无法稳定读取。
5. 根节点有子节点但 `editorFound=false` 或 `chatTitleElementFound=false` 时，分别检查 `editorQueryStatus`、`chatTitleQueryStatus`、`groupTitleQueryStatus` 及各自的错误码和候选数。候选数为 0 且树中 AutomationId/ClassName 大量可读，表示微信控件标识很可能已变化；AutomationId/ClassName 可读数接近 0，则表示 UIA 桥接只暴露了不完整子树。`uiaTreeControlTypeCounts` 用于对比正常机与故障机的无内容树结构。
6. 发送按钮问题按 `sendToolbarCount`、`sendButtonCandidateCount`、`sendButtonState` 逐级判断：工具栏为 0 是工具栏锚点缺失，候选为 0 是“发送”按钮结构/名称未命中，`query-failed` 则按 `uiaErrorCode` 排查查询失败。
7. 对比 `environment.json` 中 `weixinInstallation`：`dllCandidates` 给出安装目录内各份 DLL 的无路径指纹，`loadedModuleScanStatus=found` 时 `loadedWeixinDll` 表示该微信进程实际加载的版本；若 `loadedWeixinDllMatchesSelectedCandidate=false`，优先排查自定义目录、升级残留或启动器加载差异。若实际加载 DLL 的哈希一致而 UIA 树不同，再排查系统环境或微信运行状态。

## 隐私与恢复

1. 检查设置和审计文件，确认审计行不包含草稿文本或聊天标题；确认环境条目包含应用、Windows 和微信版本，发送链路可用匿名 `traceId` 串联。
2. 在“通用设置”导出诊断包，确认其中不包含消息、会话名称、设置文件或完整用户路径；验证“清空日志”和自动保留清理不影响设置。
3. 在“关于”页手动检查更新：验证无更新、发现更新、忽略当前版本后不再自动弹窗但仍能手动下载升级，以及 SHA-256 不匹配时不会启动安装器。
4. 关闭微信后重启应用，验证托盘显示不可用，不影响无关应用。
5. 让微信以更高权限运行而守护不提升，验证失败关闭并能表达无法保护的状态。
6. 卸载后安装升级候选包，验证 `%LocalAppData%\WeChatSendGuard` 设置和日志仍在，除非用户明确删除。

## macOS 平台追加清单

以下项目与上面的公共发送保护步骤一起执行，不能用 Windows 结果代替：

1. 使用已签名 `.app` 首次启动，分别验证“辅助功能”和“输入监控”未授权、授权、撤销及重新授权；任何未授权状态都不得补发输入。
2. 用 `codesign -d --verbose=4` 记录候选应用和官方微信的 bundle、Team ID、Hardened Runtime；验证同名但签名不匹配的测试应用不会被信任。
3. 验证通用设置只读显示 `com.tencent.xinWeChat` 与 Team `5A4RE8SF68`，不会读取或保存 Windows 可执行路径作为 macOS 信任规则。
4. 手工加入一个联系人和一个群聊，确认 Accessibility 标题及类型分类稳定；成员数量变化后标题仍能匹配。若任一类型被误判或不可识别，本候选不能声明功能一致。
5. 分别用主键盘 Return、数字键盘 Enter、发送按钮和 Esc 完成公共发送保护清单；检查补发事件只产生一条消息，且不会再次触发自身 event tap。
6. 在微信单窗口、多窗口、窗口移动/缩放、不同显示器和会话快速切换下重复测试；识别更新期间允许安全阻止，不允许原始发送漏过。
7. 启用“登录 macOS 时启动”，检查当前用户 LaunchAgent 内容、下次登录后台启动和状态栏恢复；关闭后确认 LaunchAgent 被移除，不请求管理员权限。
8. 检查 `~/Library/Application Support/WeChatSendGuard` 的设置和日志边界，验证 Finder 打开目录、JSON 导入导出、诊断 ZIP、清空日志和保留清理。
9. 从 universal DMG 安装并运行 Apple Silicon 与 Intel（或 Rosetta/Intel 测试机）候选，记录 `lipo -info`、`codesign --verify --deep --strict`、`spctl` 和 stapler 验证结果。
10. 在“关于”页验证 macOS 更新只选择 `WeChatSendGuard-<VERSION>-universal.dmg` 及同名 `.sha256`，校验失败不打开 DMG。

## 签核

将完成的清单和所有失败或跳过项按平台归档到发布记录。任一失败或跳过项都不能用于声明该平台版本的微信兼容性或 Windows/macOS 功能一致。
