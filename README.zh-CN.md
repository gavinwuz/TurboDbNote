# TurboDbNote

[English](README.md) | 简体中文

基于 Rust + GPUI Component 的原生 SQL Notebook 工作台。

当前版本：**v0.1.0（预览版）**。参见[更新日志](CHANGELOG.zh-CN.md)。

产品显示名称为 **TurboDbNote**，桌面包和可执行程序统一命名为 `turbodbnote`（Windows 为 `turbodbnote.exe`）。内部领域/服务包保留 `turbodbn-core`、`turbodbn-services`。

当前为初始工程骨架：四区 Dock 布局、右侧多 Tab、独立 SQL / Markdown / 任务编辑实体、折叠与任务状态切换，以及可无窗口测试的文档和查询状态模型。

布局采用极简编辑器：48px 纯图标导航栏包含数据笔记标识、项目、连接、设置和帮助；顶部提供三个区域开关、专注模式及 Dark / Light 切换。底部输出默认收起。主题在当前会话中生效，尚未持久化。

40px 统一标题栏将数据笔记标识、产品名称、布局/主题操作及窗口控制放在同一行，导航栏从下方开始，替代原生标题栏与工具栏两行结构。参见[标题栏设计](docs/plans/2026-09-11-unified-titlebar.md)。

> 当前示例编辑仅保留在内存中，关闭窗口后丢失。数据库、文件保存、AI、Markdown 预览和查询结果表格尚未接入；不会执行示例 SQL 或启动 AI 进程。

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

领域层不依赖 GPUI。服务层目前仅定义接口，没有假实现。桌面层使用真实 GPUI Component 控件，并保持每个编辑器实体在渲染间存活。SQL 高亮、补全和格式化需要后续注册方言及提供服务，当前不宣称已经具备。

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

同一份 Release 程序生成 `*-portable.zip`、`*-setup.exe` 和 SHA256。安装版默认安装到当前用户 Programs 目录，提供中英文向导、开始菜单快捷方式、可选桌面快捷方式及卸载入口。安装/升级前需自行退出程序，不会自动关闭。绿色版当前指免安装，配置持久化尚未实现。包内双语发布说明从根目录 CHANGELOG 生成；标签流水线上传两种候选制品，不公开发布。完整方案见[打包发布方案](docs/plans/2026-09-11-release-v0.1.0.md)。

下载后解压到独立目录，运行 `turbodbnote.exe`，保留许可证及说明，不要在程序运行时覆盖 EXE。使用 `Get-FileHash <下载文件> -Algorithm SHA256` 对照 `SHA256SUMS.txt`。当前候选版本未签名；若系统阻止运行，不要关闭系统保护，请先确认来源。反馈问题时提供 Windows 版本、GPU、复现步骤和 `BUILD-INFO.json`，不要附带密码、连接串或敏感查询。

## 文档维护

英文与中文 README 保持章节、命令、版本及功能范围同步。`CHANGELOG.md` 和 `CHANGELOG.zh-CN.md` 使用相同的 `## [Unreleased]` / `## [0.1.0]` 版本标题，最新版本放在前面；版本内部使用三级标题。`docs/plans/` 暂保留中文。不要手工维护额外的逐版本发布说明源文件。
