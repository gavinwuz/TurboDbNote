# TurboDBNote

笔记入口提供最近文件，侧栏文件缩进显示、项目拖动排序与数据库共用。新增 SQL/MD 标识图标和独立宽度的数据库搜索下拉；独立有界 JSON 文件保留原子保存，解析不再构建并克隆完整 JSON 树。

设置支持字体预览、数据库默认分页、SQL 换行与精简配置列表；连接导入支持勾选和 DBeaver DBP。详见 [设置与常用操作](docs/settings-workbench.md)。

[English](README.md) | 简体中文

基于 Rust + GPUI Component 的原生 SQL Notebook 工作台。

当前版本：**v0.1.0（预览版）**。参见[更新日志](CHANGELOG.zh-CN.md)。

产品显示名称为 **TurboDBNote**，桌面包和可执行程序统一命名为 `turbodbnote`（Windows 为 `turbodbnote.exe`）。内部领域/服务包保留 `turbodbn-core`、`turbodbn-services`。现有数据目录和凭据标识沿用历史拼写，以兼容已有配置。

当前为初始工程骨架：四区 Dock 布局、右侧多 Tab、独立 SQL / Markdown / 任务编辑实体、折叠与任务状态切换，以及可无窗口测试的文档和查询状态模型。

布局采用极简编辑器：48px 纯图标导航栏包含数据笔记标识、项目、连接、设置和帮助；顶部提供三个区域开关、专注模式及 Dark / Light 切换。底部输出默认收起。主题修改后自动保存。

40px 统一标题栏将数据笔记标识、产品名称、布局/主题操作及窗口控制放在同一行，导航栏从下方开始，替代原生标题栏与工具栏两行结构。参见[标题栏设计](docs/plans/2026-09-11-unified-titlebar.md)。

> 笔记使用独立 `.note` 文件，未命名草稿也落盘；左侧笔记与数据库共用项目分类。SQL 子区域使用已保存 MySQL/PostgreSQL/SQLite 连接并提供分页结果（默认 20 行），Markdown 和任务可离线使用。设置 → 通用可配置临时目录和 600ms 自动保存。参见[笔记工作台与恢复边界](docs/note-workbench.md)。已保存配置时，AI Chat 首次显示可自动检查所选 CLI；只有主动发送消息才发起模型请求。

Note 单元默认阅读，双击/F2 编辑、Esc 返回阅读；编辑工具栏提供复制、移动和前后插入。SQL 结果复用表数据网格，含完整主键的单表普通列查询支持确认编辑，关联及计算结果保持只读。

Note 顶部采用紧凑图标工具条；未指定连接的 SQL 单元继承默认连接/库，显式单元配置保持独立。全部执行按顺序进行并逐项确认写入。重命名、另存为、项目及打开位置放入 Note 右键菜单，页签圆点与提示显示保存状态和文件信息。

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

Windows 资源还包含 VERSIONINFO，提供 FileDescription / ProductName（TurboDBNote）以及文件版本。发布更新版本时同步修改 `assets/windows.rc` 的版本字段。任务管理器的进程分组与窗口子项可能分别缓存名称和图标，更新后需重启应用及任务管理器检查。

图标兼容性：16–128px 使用传统 32 位 DIB + AND mask，256px 使用 PNG。Windows 启动时还显式设置 HWND 的 `ICON_SMALL` / `ICON_BIG`，补充 GPUI 的窗口类图标。资源验证脚本会实际调用 Windows 图标解码 API；`python scripts/verify-window-icons.py` 可启动独立空白实例验证两个窗口图标槽并关闭该实例。

## MySQL 连接管理

左侧数据库图标可新建、搜索连接，在中心标签页编辑项目、地址、账号、默认库、驱动/版本提示、UTF-8 会话字符集、TLS 证书和超时。测试完成释放连接，“连接”保留管理会话直至断开；建立持久连接前需先保存配置。双击连接名连接；右键统一连接/断开、编辑、复制、记录和删除。复制配置不复制密码，删除需要确认。

现代适配器使用 SQLx 0.8.6，目标为 MySQL 5.6/5.7/8.0；4.0/4.1/5.0–5.5 使用 Legacy 兼容路由，由独立辅助进程加载预装/托管客户端，或明确指定的 MySQL 5.1–5.6 x64 `libmysql.dll`。自动路由依据所选目标版本，不会在认证失败后降级重试。实际检测版本与目标提示分别显示。TLS 默认验证身份，不静默降级。密码只留当前内存，或选择保存到 Windows DPAPI 加密存储；`connections.db` 保存连接配置及创建、修改、最近使用时间，密码仅以密文保存；首次导入旧 `connections.json` 并保留原文件。管理连接现支持表、字段及索引浏览；Notebook SQL 使用独立后台会话，写入需要预览并确认。详见[实现方案](docs/plans/2026-09-17-mysql-connections.md)。

连接存储位置：安装版为 `%APPDATA%\TurboDbNote\connections.db`，绿色版为程序旁的 `data/connections.db`。编辑时密码框留空表示沿用，替换与清除明确区分；启用 Windows DPAPI 加密存储可在重启后继续使用，修改主机、端口或用户名则需重新输入密码。对话框操作栏统一右对齐，删除及 SQL 风险写入需二次确认。详见[连接存储与交互设计](docs/connection-storage-and-feedback.md)。

数据页采用行列虚拟表格与固定行号，支持主键游标分页、服务端排序筛选、按需总数统计和多格式行复制。编辑先暂存，再预览 SQL、确认并以事务提交；提交时校验原始行，回滚按钮仅撤销未提交修改。详见[数据工作台设计](docs/data-grid-workbench.md)。

数据库标题栏提供最近连接和 Navicat NCX、DBeaver JSON/ZIP 导入；建库按驱动提供字符集选项，MySQL/SQLite 支持复制到新目标，数据可选。详见[数据库管理方案与范围](docs/database-management.md)。

筛选支持最多 20 条条件，以 AND 或 OR 统一组合。日期、时间分别输入并提供当前时间；单元格编辑提供设为 NULL、恢复原值、按方言转义的复制 WHERE 条件。

筛选条件常驻表格顶部，展开后以符号运算符编辑；日期提供日历并保留时间精度、时区。默认浏览，双击/F2 编辑单元格，Ctrl+C 复制单元格值。

分页默认每页 200 行（可输入 1–1000），底栏提供首页、上一页、页码、下一页、尾页；总行数仅手动统计，统计后启用尾页。复制新增与导出直接显示为图标按钮。

表格操作与统计整合为底部单行工具条。默认高亮随主题切换：浅色用暖黄，深色用柔和蓝灰；自定义颜色优先保留。

单击连接仅切换库对象，不打开配置页；通过右键 **编辑连接** 进入配置。库名旁仅保留离线/加载状态，不再重复显示表数与视图数。

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

### 连接旧版 MySQL

选择旧版目标版本（或 Legacy 驱动），浏览选择可信的 MySQL 5.1–5.6 **x64** `libmysql.dll`，相关依赖 DLL 保持在同目录。发行打包现默认包含已验证的 5.1.73 x64 客户端及许可证、源码。自定义 DLL 路径留空即可使用预装或托管驱动。ZIP 和安装版会包含 `turbodbnote-mysql-legacy.exe`，需与主程序保持同目录。

兼容连接当前要求明确禁用 TLS，请使用适当的受保护网络或隧道。MySQL 4.0 选择 `server-default` 字符集，4.1/早期 5.x 可选择 `utf8` 或 `server-default`。低于 5.6 的客户端必须明确勾选允许旧密码认证，因为其 secure-auth 开关不能可靠覆盖认证切换。现代驱动仍默认验证 TLS 身份。

辅助进程隔离原生驱动崩溃，连接有超时限制，取消/断开会结束辅助进程。密码通过标准输入传递，不进入命令行。可运行 `scripts/verify-mysql-legacy.ps1 -ServerDirectory <解压的服务端目录> -ClientDirectory <解压的x64客户端目录>` 复现测试；脚本只在 `target` 下创建独立本机实例，不修改已有数据库。

已使用 5.1.73 x64 客户端实测 MySQL 4.0.27、4.1.22、5.1.73：连接、断开、错误密码、旧认证允许/拒绝以及 TLS 拒绝均通过；其它 5.x 版本及 5.6 客户端尚未实测。

### 驱动管理与连接表单

「设置 → 数据库」支持官方下载（约 119 MiB）、离线导入 `mysql-noinstall-5.1.73-winx64.zip`、进度、取消及重试。归档和 DLL 均固定 SHA-256 校验，未完成下载不会成为可用驱动。自定义 DLL 仍可在各连接「高级」页指定。下载的驱动存入设置目录旁的 `drivers`，预装驱动存入应用旁的 `drivers`，重启后优先使用预装项；不修改 PATH。

连接表单拆分为「连接信息」「TLS / SSL」「高级」，底部固定测试、保存并连接等操作。环境预设开发、测试、预发、正式，可直接自定义输入；项目下拉建议已保存连接中的项目，也可输入新项目。TLS 和旧认证仍需明确选择。

Windows 打包默认携带用于连接 4.x 的 5.1.73 x64 客户端，不携带无法由 x64 程序加载的 win32 4.x DLL。默认从官方获取客户端和对应源码；离线打包在 `scripts/package-windows.ps1` 增加 `-LegacyDriverArchive <官方ZIP> -LegacySourceArchive <mysql-5.1.73.tar.gz>`。发行目录保留 MySQL 源码和当前应用源码，不安装或启动 MySQL 服务。

### 数据库检测与字符集

连接字符集支持 UTF-8、GB2312、GBK 预设及 MySQL 字符集名称自由输入，实际可用性取决于服务端/客户端。MySQL 4.0 不支持按会话协商字符集，仍需 `server-default`。较新服务端的元数据临时按 UTF-8 读取，完成后恢复所选结果字符集。

数据库输入框旁「检测」会列出当前账号可见的数据库，不保存草稿、不改动已经填写的库名；下拉可选择，也可手动输入。左侧连接下提供可折叠列表：已指定库名时仅显示该库，留空则显示检测得到的可见库。检测最多列出 512 项，权限/编码问题会提示；连接参数变化后旧建议失效。历史服务端返回的无效编码库名会提示并跳过，避免静默产生乱码。

常用操作采用 icon + 文字，撤销/复制/删除收进「更多」。连接详情、驱动路径默认收起；驱动详情提供「复制路径」「打开位置」。

### TLS 连接引导

现代连接默认「加密并验证身份」，另提供「仅加密（不验证身份）」「不加密」。本机回环地址只显示开发提示，不会根据 IP、内网或域名自动切换模式。当前兼容驱动提供明确的「切换为不加密」入口。

证书、握手和服务端缺少 TLS 能力等错误会弹出引导。证书问题可选择「仅加密测试」，也可明确选择「不加密测试」；服务器强制加密时不提供关闭加密的快捷操作。没有 TLS 依据的协议/响应格式错误不会引导降级，不自动重试。

快捷测试默认不改已保存配置。勾选「测试成功后保存当前连接及此模式」后，只有成功才保存当前草稿；失败不会保存。测试成功后可「使用此模式连接」。弹窗绑定请求时的连接快照，参数变化后的旧操作不再执行。

### 多类型数据库管理

「新建连接」先选择 MySQL、SQLite、PostgreSQL、ClickHouse 或 Redis，再填写对应表单；返回切换类型时保留各自未保存草稿。原有 MySQL 配置继续读取。包含其它数据库类型时写入连接格式 v2，防止旧版程序误按 MySQL 加载；当前版本同时读取 v1/v2。

数据库侧栏上部按「项目 → 连接」组织，可切换按环境、创建日期或使用日期查看；项目排序和折叠状态保存到 `database-navigation.json`。下部库对象优先选择连接的默认数据库，并支持搜索切换其他库。搜索使用已有元数据，不自动连接。

点击已连接的数据库进入右侧「数据库对象」面板，列、索引保持在右侧。操作复用已连接适配器，携带请求序号，在后台执行；断开或适配器异常会释放等待状态。建库须在弹窗中明确提交，展示目标与环境，不自动重试；未限定单库的连接在成功后自动选中新库。

| 适配器 | 浏览能力 | 创建方式 |
| --- | --- | --- |
| MySQL（SQLx / 兼容 C 客户端） | 表、视图、列、索引 | 服务端建库 |
| SQLite | 表、视图、列、索引；默认只读 | 明确新建文件，不覆盖已有文件 |
| PostgreSQL | 带 Schema 的表/视图、列、索引 | 服务端建库 |
| ClickHouse HTTP/HTTPS | 表/视图、列、跳数索引 | 服务端建库 |
| Redis | 逻辑库、增量 SCAN、Key 类型/TTL/长度 | 不提供建库；集群仅 DB 0，浏览当前节点 |

ClickHouse 使用 HTTP 端口（常见不加密 8123 / TLS 8443），不使用原生 TCP 端口。Redis 逻辑库枚举可能受 CONFIG 权限限制，可手动填写编号。SQL 对象列表最多 500 项，数据库目录最多 512 项。

验证使用了真实 SQLite 3.46.0、PostgreSQL 16.4、Redis 5.0.14.1 和 MySQL 实例。ClickHouse 已通过本地 HTTP 契约测试，但当前 Windows 没有可用的真实 ClickHouse 服务端，尚未实连验收。`scripts/verify-database-adapters.py` 支持显式 Redis/PostgreSQL 程序路径及 `--http-contract`，仅创建独立本机测试实例。

## 布局与设置中心

标题栏操作与最左侧导航仅显示图标并提供提示；内容区按钮和设置分组使用图标＋文字。页签显示图标、标题和关闭图标，编辑区更多菜单和右键菜单支持关闭全部；右键另支持关闭当前、其它、右侧、左侧。右侧 AI 对话、检查器、表结构为不可关闭的固定页签，顺序保持不变。关闭保留 Notebook 和聊天实体；帮助 → 恢复辅助页签可重新打开。设置打开时暂时收起辅助区域，关闭后恢复原可见状态。

设置修改停止 600ms 后自动保存有效内容，不再提供保存按钮；关闭页签或窗口时刷新待保存内容。校验失败或写入失败保留草稿，并提供重试、放弃、继续编辑。自动保存不会连接智能体或发送推理请求。通用包含白天/黑夜与扩展主题、语言、字体和字号；AI 智能体保留 Codex/Claude 配置，新增 OpenClaw/扩展登记、可执行文件、工作目录与显式安装指南入口。扩展条目目前用于配置管理，内置聊天仍使用 Codex/Claude。

AI 提供商可保存多套具名配置，支持 OpenAI、Gemini、DeepSeek、Qwen、Ollama、Custom。可选 Chat Completions、Responses、Anthropic Messages，填写 API 基地址（例如 `/v1`）、遮罩 API Key、模型 ID 与高级 JSON 参数。获取模型列表只在点击时请求对应 `/models`，也可手填模型 ID。这些配置尚未作为直接 API 聊天通道使用。切换厂商会清除前一厂商的密钥和模型；除本机回环地址外要求 HTTPS，不跟随重定向，请求超时五秒。

配置改为 SQLite `settings.db`，通过事务保存完整设置文档。首次使用导入同目录旧 `settings.json`，保留原文件作备份；便携版路径为 `data/settings.db`，安装版使用用户配置目录。API Key 在界面遮罩显示，但本地数据库未加密。聊天快照仍单独使用 JSON。

独立语言包位于 `crates/desktop/assets/locales/`。将兼容 JSON 包放到 `settings.db` 同目录的 `locales/`，重启后可新增或覆盖语言，再到通用中选择。格式为 `schema_version: 2`、`id`、`name` 与扁平 `strings` 字典，键采用稳定英文语义标识；缺失项依次回退同语言内置包、内置英文。本次迁移保留旧原文键兼容，详见[多语言规范与校验](docs/i18n.md)。扩展主题使用 GPUI Component 的 ThemeSet JSON 格式，放入相邻 `themes/` 目录后在“扩展主题”选择。

帮助提供关于、GitHub 与后台检查最新稳定版。关于以信息卡展示版本、发布日期、通道、平台和许可，发布元数据维护在 `crates/desktop/assets/release.json`；当前预览版尚未正式发布，因此不虚构发布日期。更新通过 GitHub latest-release API 和语义版本比较检查，不自动下载或安装。详见[设计方案与验收](docs/layout-settings.md)。

### 紧凑 AI 工作区

输入区上方显示禁用的连接/数据库选择位，等待数据库上下文接入聊天；对话操作工具栏只显示图标并提供提示，模型选择保留当前名称，过长时截断。智能体设置直接显示 Codex、Claude、OpenClaw Tab；Codex/Claude 切换当前引擎，各自配置和历史独立，生成、连接或有效配置尚待保存时禁止切换。OpenClaw 仍为配置登记入口。

字号统一关联通用设置：正文/控件 1 倍、辅助说明 0.875 倍、分组标题 1.125 倍、品牌标题 1.25 倍；代码采用相同基准字号，主题重载保留用户设置。空对话不显示最新/恢复，账号检查成功仅保留在运行日志，聊天区继续显示必要的错误与生成进度。

底部面板标题为终端图标＋**输出**，跟随、清空、关闭集中在标题右侧图标工具条。日志显示本地时间戳，明确错误带 ERROR 标识和错误色；状态栏左侧显示数据库状态，右侧显示实时 AI 状态。

## CrashSight 诊断

已实现 Windows 动态加载、macOS Objective-C 桥接源码及独立有界 SQLite 使用事件队列。海外项目优先，国内备用仅在确认失败并写入标记后于下次启动选择。**公开 C API 没有可用的上传失败结果通知，自动失败判定尚未接通。** 客户端不嵌入 App Key。官方 Windows SDK 已下载到忽略的 vendor 目录并只读检查，未初始化；macOS 仍需原生 framework 和真机编译。

SDK 缺失不影响应用运行，仅保留固定类型的启动/动作等本地记录。业务上传需要自有 HTTPS 接收服务，目前未配置。原生崩溃 dump 可能包含内存和设备信息。详见[配置、边界、符号文件与验证](docs/crashsight-integration.md)。

保存的数据库密码使用当前 Windows 用户 DPAPI 加密，密文存入 connections.db；旧系统凭据自动迁移。存储记录不显示路径，可点击“打开所在文件夹”。复制到其他电脑或用户后需重新输入密码。

数据库支持可搜索切库，以及左右来源/目标布局的导出、复制和导入任务，提供带时间的导出名、选表、网格预览、分批参数、确认、进度与取消；SQLite 与现代 MySQL 仅数据导入支持追加、覆盖及主键更新，详见[数据库传输工作台](docs/database-transfer.md)。

导出支持文件、新库及已有库；复制至新库遇到重名即停止，预检核对实际执行通道。底部输出有界跟踪操作，并分卷保存结构化日志。关闭或切换未保存连接编辑页时，明确选择继续编辑或放弃修改。

开发预览复用 `target/debug` 与 `target/preview`：执行 `pwsh -File scripts/build-preview.ps1`。使用 `pwsh -File scripts/clean-target.ps1` 查看可清理空间，加 `-Apply` 清理旧二进制和增量缓存，保留笔记、数据库及配置；发行打包复用带互斥锁的隔离源码目录。
