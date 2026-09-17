# TurboDbNote

[English](README.md) | 简体中文

基于 Rust + GPUI Component 的原生 SQL Notebook 工作台。

当前版本：**v0.1.0（预览版）**。参见[更新日志](CHANGELOG.zh-CN.md)。

产品显示名称为 **TurboDbNote**，桌面包和可执行程序统一命名为 `turbodbnote`（Windows 为 `turbodbnote.exe`）。内部领域/服务包保留 `turbodbn-core`、`turbodbn-services`。

当前为初始工程骨架：四区 Dock 布局、右侧多 Tab、独立 SQL / Markdown / 任务编辑实体、折叠与任务状态切换，以及可无窗口测试的文档和查询状态模型。

布局采用极简编辑器：48px 纯图标导航栏包含数据笔记标识、项目、连接、设置和帮助；顶部提供三个区域开关、专注模式及 Dark / Light 切换。底部输出默认收起。主题修改后自动保存。

40px 统一标题栏将数据笔记标识、产品名称、布局/主题操作及窗口控制放在同一行，导航栏从下方开始，替代原生标题栏与工具栏两行结构。参见[标题栏设计](docs/plans/2026-09-11-unified-titlebar.md)。

> Notebook 编辑仅保留在内存中，关闭窗口后丢失。数据库执行、文件保存、笔记 Markdown 预览和查询结果表格尚未接入。已保存配置时，AI Chat 首次显示可自动检查所选 CLI；只有主动发送消息才发起模型请求。

## 开发环境

- Rust 1.92.0（本次验证工具链），Cargo；固定依赖见 `Cargo.lock`。
- Windows：MSVC 工具链、Visual Studio C++ Build Tools 与 Windows SDK、CMake。GPUI 依赖原生图形环境。
- macOS / Linux：需安装 [GPUI 对应平台依赖](https://github.com/zed-industries/zed/tree/main/docs/src/development)，本次主要验证 Windows。
- `gpui = 0.2.2` / `gpui-component = 0.5.1` / 内置图标 assets 使用匹配版本；升级时整体检查，勿混用 gpui-pre 类型。

```powershell
cargo run --locked -p turbodbnote
```

首次编译会下载并构建 GPUI 原生依赖，耗时明显长于增量构建。Release 性能验证使用 `cargo run --locked --release -p turbodbnote`。

## 工程结构

```text
crates/
  core/src/        # .note schema、子项类型、独立查询状态
  services/src/    # 数据库游标、AI 会话与类型化数据契约
  desktop/src/    # 应用启动、Dock 工作台、子项编辑器
docs/plans/       # 中文开发方案、架构取舍及验收目标
```

领域层不依赖 GPUI。服务层包含数据库契约及实际 Codex App Server 适配。桌面层使用真实 GPUI Component 控件，并保持每个编辑器实体在渲染间存活。SQL 高亮、补全和格式化需要后续注册方言及提供服务，当前不宣称已经具备。

## 验证

```powershell
cargo fmt --all -- --check
cargo test --locked -p turbodbn-core -p turbodbn-services
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
```

人工验收：窗口打开后切换右侧三个标签；调整各区域尺寸；顶部按钮隐藏/恢复区域；编辑 SQL 和笔记后折叠/展开，文本保持；点击任务状态循环切换。当前不开放跨区域拖动，以保持项目、对象和结果的区域约束。

新增布局验收：点击项目/连接切换侧栏，重复点击收起；记录任意面板组合后进入/退出专注模式，应恢复原组合及尺寸；通过顶部和设置窗口切换 Dark / Light，检查编辑器、弹窗与图标对比度。点击帮助可查看操作说明。

产品图标采用“数据笔记”：数据库弧线 + 笔记本装订 + 页角。彩色稿见 [应用图标 SVG](crates/desktop/assets/data-note-app.svg)，导航栏使用随主题着色的 [小图标 SVG](crates/desktop/assets/icons/data-note.svg)。已嵌入导航栏与 Windows EXE（资源 ID 1），GPUI 自动使用该资源作为窗口图标。多尺寸 ICO 已提交，正常构建无需图片转换工具。

完整需求映射、数据流、性能预算、Codex 集成方案与后续开发步骤见 [Rust 工作台开发方案](docs/plans/2026-09-11-rust-workbench.md)。

本次编译、测试及界面验证边界见 [验证记录](docs/plans/2026-09-11-validation.md)。

## Windows 应用图标

`crates/desktop/build.rs` 编译 `assets/windows.rc`，将 `assets/turbodbnote.ico` 嵌入 EXE 资源 ID 1，与 GPUI 的窗口图标加载约定一致。包含 16、20、24、32、48、64、128、256px 的 32 位图像。导航 SVG 的基础描边改为青绿色，可直接在白色背景预览；GPUI 内继续按主题着色。

修改彩色 SVG 后再生成资源（仅美术资源维护需要 Python 依赖）：

```powershell
python -m pip install resvg-py==0.5.0 Pillow
python scripts/generate-icons.py
cargo build --locked -p turbodbnote
python scripts/verify-windows-icon.py
```

[尺寸及背景预览](crates/desktop/assets/icon-preview.png)。检查新图标时先退出旧进程，再启动新 EXE；已固定的任务栏快捷方式若仍显示旧图标，可取消固定后重新固定。

Windows 资源还包含 VERSIONINFO，提供 FileDescription / ProductName（TurboDbNote）以及文件版本。发布更新版本时同步修改 `assets/windows.rc` 的版本字段。任务管理器的进程分组与窗口子项可能分别缓存名称和图标，更新后需重启应用及任务管理器检查。

图标兼容性：16–128px 使用传统 32 位 DIB + AND mask，256px 使用 PNG。Windows 启动时还显式设置 HWND 的 `ICON_SMALL` / `ICON_BIG`，补充 GPUI 的窗口类图标。资源验证脚本会实际调用 Windows 图标解码 API；`python scripts/verify-window-icons.py` 可启动独立空白实例验证两个窗口图标槽并关闭该实例。

## 打包发布

目标为 Windows 10 1809+ / Windows 11 x64，不支持 Windows 7。安装 Inno Setup 6.7.3 和 Windows SDK 后通过参数指定工具路径；省略 `-FxcPath` 时从注册表和标准 SDK 目录发现。不使用自定义工具路径环境变量、不修改 PATH。临时隔离的 GPUI 源码适配从本地文件读取编译器路径，并检查版本；不修改全局 Cargo 源码或仓库锁文件。

```powershell
pwsh -File scripts/package-windows.ps1 -FxcPath 'C:\SDK\x64\fxc.exe' -IsccPath 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe'
```

同一份 Release 程序生成 `*-portable.zip`、`*-setup.exe` 和 SHA256。安装版默认安装到当前用户 Programs 目录，提供中英文向导、开始菜单快捷方式、可选桌面快捷方式及卸载入口。安装/升级前需自行退出程序，不会自动关闭。绿色 ZIP 包含 portable.flag，设置保存到程序旁 data/settings.db；安装版使用用户配置目录。包内双语发布说明从根目录 CHANGELOG 生成；标签流水线上传两种候选制品，不公开发布。完整方案见[打包发布方案](docs/plans/2026-09-11-release-v0.1.0.md)。

下载后解压到独立目录，运行 `turbodbnote.exe`，保留许可证及说明，不要在程序运行时覆盖 EXE。使用 `Get-FileHash <下载文件> -Algorithm SHA256` 对照 `SHA256SUMS.txt`。当前候选版本未签名；若系统阻止运行，不要关闭系统保护，请先确认来源。反馈问题时提供 Windows 版本、GPU、复现步骤和 `BUILD-INFO.json`，不要附带密码、连接串或敏感查询。

## AI Chat：Codex / Claude Code CLI

设置 → AI 智能体提供“打开 AI 面板时自动连接”和“启动时恢复上次会话”，按引擎保存，默认开启。仅已保存配置可自动连接，只做就绪检查，不推理或重发消息。每个引擎本次运行自动尝试一次；手动断开或失败后使用重试按钮。关闭历史恢复不会删除已保存历史。

引擎菜单支持切换 Codex App Server / Claude Code CLI。两者分别保存可执行路径、工作目录、模型、当前草稿与上次会话历史。连接、保存或生成期间禁止切换，请先停止当前轮次；切换不自动发送请求。

最近会话分别保存到设置同目录：Codex 使用 `last-chat.json`，Claude 使用 `last-chat-claude.json`。启动先恢复消息、部分输出及耗时，再独立进行连接；下一次主动发送时，相同程序/工作目录可续接该引擎保存的会话。服务端历史不存在时显示错误，可通过“新会话”重新开始。这是单个上次会话快照，不是可搜索的多会话列表。聊天内容以普通 JSON 保存在本地，强制退出可能丢失最后一个保存间隔。

设置现在作为主编辑区标签与 Notebook 并列，重复打开复用同一页。左侧入口恢复上次分类，AI Chat 的“前往设置”和齿轮直达 AI 分类。修改后延迟 600ms 自动保存，关闭前刷新；保存失败可重试、放弃或继续编辑，Notebook 和聊天实体保持存活。

AI Chat 使用更宽的面板，以输出为主。首次点击“前往设置”：统一设置标签分为“通用 / AI”，AI 分类包含所选引擎的自动检测、可执行文件、工作目录、账号连接及默认模型。连接后开始对话，之后重启打开面板默认自动连接；检测不发送模型请求。通过引擎菜单选择 Codex 或 Claude；Claude 原生程序检测检查 PATH 与用户 .local/bin 目录。

聊天顶部提供“＋ 新会话”、扩大/恢复和设置。生成时发送按钮变为方形停止图标，并显示耗时；中断保留已收到内容，协议取消超时后结束后台进程树。向上滚动暂停跟随，“最新”恢复；点击选择正文时冻结显示快照，后台继续接收。Markdown 支持选择、链接和代码块复制；异步解析恰好完成时的选区边界仍需 UI 验收。

设置按提供方持久化，上次 UI 聊天记录保存在本地，Codex 自身也可能保留历史。保存 AI 配置不启用数据库执行。Notebook 引用/插入、多会话历史列表及应用内登录留待后续实现。详见[聊天布局设计](docs/plans/2026-09-14-chat-layout.md)。

已用 Codex CLI 0.153.4 与 Claude Code CLI 2.1.139 验证。Claude 使用 print-mode 流式 JSON 和 --resume，复用 CLI 登录、禁用工具，停止时终止当前轮次进程。Claude 模型采用 CLI 官方别名（默认 / sonnet / opus / haiku），不是实时账号模型目录，本版不覆盖 Claude 推理强度。参见[Claude 集成设计](docs/plans/2026-09-17-claude-cli.md)。Codex 详见[集成设计](docs/plans/2026-09-11-codex-app-server.md)。工具路径通过进程 API 参数传入，模型和推理强度通过协议请求传递，不设置自定义工具路径环境变量。

## 文档维护

英文与中文 README 保持章节、命令、版本及功能范围同步。`CHANGELOG.md` 和 `CHANGELOG.zh-CN.md` 使用相同的 `## [Unreleased]` / `## [0.1.0]` 版本标题，最新版本放在前面；版本内部使用三级标题。`docs/plans/` 暂保留中文。不要手工维护额外的逐版本发布说明源文件。

## 布局与设置中心

标题栏操作与最左侧导航仅显示图标并提供提示；内容区按钮和设置分组使用图标＋文字。页签显示图标、标题和关闭图标，编辑区更多菜单和右键菜单支持关闭全部；右键另支持关闭当前、其它、右侧、左侧。右侧 AI 对话、属性、表结构为不可关闭的固定页签，顺序保持不变。关闭保留 Notebook 和聊天实体；帮助 → 恢复辅助页签可重新打开。设置打开时暂时收起辅助区域，关闭后恢复原可见状态。

设置修改停止 600ms 后自动保存有效内容，不再提供保存按钮；关闭页签或窗口时刷新待保存内容。校验失败或写入失败保留草稿，并提供重试、放弃、继续编辑。自动保存不会连接智能体或发送推理请求。通用包含白天/黑夜与扩展主题、语言、字体和字号；AI 智能体保留 Codex/Claude 配置，新增 OpenClaw/扩展登记、可执行文件、工作目录与显式安装指南入口。扩展条目目前用于配置管理，内置聊天仍使用 Codex/Claude。

AI 提供商可保存多套具名配置，支持 OpenAI、Gemini、DeepSeek、Qwen、Ollama、Custom。可选 Chat Completions、Responses、Anthropic Messages，填写 API 基地址（例如 `/v1`）、遮罩 API Key、模型 ID 与高级 JSON 参数。获取模型列表只在点击时请求对应 `/models`，也可手填模型 ID。这些配置尚未作为直接 API 聊天通道使用。切换厂商会清除前一厂商的密钥和模型；除本机回环地址外要求 HTTPS，不跟随重定向，请求超时五秒。

配置改为 SQLite `settings.db`，通过事务保存完整设置文档。首次使用导入同目录旧 `settings.json`，保留原文件作备份；便携版路径为 `data/settings.db`，安装版使用用户配置目录。API Key 在界面遮罩显示，但本地数据库未加密。聊天快照仍单独使用 JSON。

独立语言包位于 `crates/desktop/assets/locales/`。将兼容 JSON 包放到 `settings.db` 同目录的 `locales/`，重启后可新增或覆盖语言，再到通用中选择。格式为 `id`、`name`、`strings` 字典，缺失翻译回退原文。扩展主题使用 GPUI Component 的 ThemeSet JSON 格式，放入相邻 `themes/` 目录后在“扩展主题”选择。

帮助提供关于、GitHub 与后台检查最新稳定版。关于以信息卡展示版本、发布日期、通道、平台和许可，发布元数据维护在 `crates/desktop/assets/release.json`；当前预览版尚未正式发布，因此不虚构发布日期。更新通过 GitHub latest-release API 和语义版本比较检查，不自动下载或安装。详见[设计方案与验收](docs/layout-settings.md)。

### 紧凑 AI 工作区

输入区上方显示禁用的连接/数据库选择位，等待数据库功能接入；对话操作工具栏只显示图标并提供提示，模型选择保留当前名称，过长时截断。智能体设置直接显示 Codex、Claude、OpenClaw Tab；Codex/Claude 切换当前引擎，各自配置和历史独立，生成、连接或有效配置尚待保存时禁止切换。OpenClaw 仍为配置登记入口。

字号统一关联通用设置：正文/控件 1 倍、辅助说明 0.875 倍、分组标题 1.125 倍、品牌标题 1.25 倍；代码采用相同基准字号，主题重载保留用户设置。空对话不显示最新/恢复，账号检查成功仅保留在运行日志，聊天区继续显示必要的错误与生成进度。

底部面板标题为终端图标＋**输出**，跟随、清空、关闭集中在标题右侧图标工具条。日志显示本地时间戳，明确错误带 ERROR 标识和错误色；状态栏左侧显示数据库状态，右侧显示实时 AI 状态。
