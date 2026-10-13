# TurboDBNote

[English](README.md) | [简体中文](README.zh-CN.md)

基于 Rust 与 GPUI 的原生 SQL Notebook 和数据库工作台，将 SQL、Markdown、任务笔记、数据库浏览及 AI 对话放在一起。

**版本 0.1.0 · Windows 与 Linux x64**

[下载](https://github.com/gavinwuz/TurboDbNote/releases) · [更新日志](CHANGELOG.zh-CN.md) · [发布流程](docs/releasing.zh-CN.md)

## 主要功能

- **数据笔记：** 独立 `.note` 文件、项目分类、最近文件、恢复及 600ms 自动保存；SQL 单元可继承或覆盖连接／数据库。
- **SQL 结果：** 分页网格、JSON 视图、影响行数摘要、筛选、排序、检查器及可读 INSERT/UPDATE 复制；写入须预览和明确确认。
- **Markdown 与任务：** 图标＋提示格式工具、预览及撤销；清单支持负责人、截止时间、完成状态和本地提醒。
- **数据库工作台：** 连接管理、对象／表结构浏览，按驱动能力建库及导入／导出／传输，对象选择及排序规则。
- **AI 辅助：** Codex、Claude Code CLI 对话，独立会话／设置、流式输出及停止；SQL 执行和分享需明确授权。
- **桌面体验：** 可调整面板、深浅主题、中英文、字体预览及结构化操作日志；Windows 支持记住关闭行为的托盘托管与打开／退出。

## 支持的数据库

提供 16 种驱动：

| 分类 | 数据库 |
| --- | --- |
| 关系型／本地 | MySQL、PostgreSQL、SQLite、MariaDB、TiDB、SQL Server、Oracle、Microsoft Access、IBM DB2 for LUW |
| 分析／时序 | ClickHouse、Apache Doris、TDengine |
| NoSQL／搜索 | Redis、MongoDB、Cassandra、Elasticsearch |

能力依驱动而异。Note SQL 执行支持 MySQL/PostgreSQL/SQLite；外部适配器提供连接、对象浏览与有界只读预览，Doris 为只读。Access/DB2 需对应 ODBC 提供程序，Oracle 需 OCI 客户端。详见[驱动能力与前置条件](docs/database-drivers.md)。

## 安装

- **Windows 10 1809+／Windows 11 x64：** 解压便携 ZIP 后运行 `turbodbnote.exe`，保留同目录的 `drivers` 与许可／源码材料。不支持 Windows 7，程序尚未签名。
- **Linux x64：** 解压 tar.gz 后运行 `./turbodbnote`；以 Ubuntu 22.04／glibc 2.35 为基线，需正常的 X11/Wayland 桌面、Vulkan 图形驱动、fontconfig、libxkbcommon 与 unixODBC。依赖详见包内 `*-dependencies.txt`。
- 下载后使用 `SHA256SUMS.txt` 校验。AI 需另外安装并登录 Codex 或 Claude Code CLI，发行包不包含这些 CLI。

笔记保存为独立 `.note` 文件，设置、导航和连接元数据使用 `connections.db`。Windows 便携版数据位于程序旁的 `data`，Linux 使用当前用户配置目录。Windows 密码使用当前用户 DPAPI 加密；Linux 密码仅保留在会话内存，重启后需重新输入。Windows 加密凭据不能直接迁移给其它用户或 Linux 使用。

托盘、SQL Server 集成认证及随包旧版 MySQL DLL 仅 Windows 提供。Redis 集群浏览限当前连接节点；提醒仅在应用运行且笔记已打开时有效。不提供自动更新。

## 开发

使用 Rust **1.92.0**、Cargo 与锁定依赖。GPUI、Component 和 assets 配套固定版本；Windows 需 Visual Studio C++ Build Tools、Windows SDK、CMake 和 PowerShell 7；Linux 原生依赖列在发布工作流中。

```powershell
cargo run --locked -p turbodbnote
```

Windows 可运行 `./scripts/build-preview.ps1` 更新固定 `target/preview` 预览包与驱动组件。发行使用隔离 GPUI 构建适配，不修改全局 Cargo registry。

```powershell
cargo fmt --all -- --check
cargo check --locked -j 1 --workspace --all-targets
cargo test --locked -j 1 --workspace -- --test-threads=1
cargo clippy --locked -j 1 --workspace --all-targets -- -D warnings
```

串行测试用于避免 Windows 凭据测试互相干扰。CI 检查原生构建、隔离协议及发布包窗口启动；X11/Xvfb 启动不代表完整 Linux 桌面／GPU／Wayland 验收，也不代表所有数据库真实服务端已验收。原生崩溃报告投递尚未验证。

## 文档

- [笔记工作台与恢复](docs/note-workbench.md)
- [数据库传输](docs/database-transfer.md)
- [设置与常用操作](docs/settings-workbench.md)
- [数据库驱动范围](docs/database-drivers.md)
- [诊断与验证边界](docs/crashsight-integration.md)
- [版本发布与打包](docs/releasing.zh-CN.md)

下次发布时，在双语 Unreleased 中补充说明，将功能提交到干净的默认分支，再运行 `./scripts/release.ps1 -Version 0.1.1`（Linux：`python3 scripts/prepare-release.py 0.1.1`）。Actions 仅在两平台验证通过后打包发布；版本标签不可变，不覆盖已有 Release。

采用 [Apache-2.0](LICENSE) 许可，随包第三方组件保留各自许可。

后续 Windows 版本同时发布 Inno Setup 安装包、绿色 ZIP 与 SHA256 校验，详见[发布流程](docs/releasing.zh-CN.md)。当前程序仍未签名。

[隐私政策](PRIVACY.zh-CN.md)

现代 MySQL 版本路由支持 5.6、5.7、8.x、9.x（含 8.4.5）；允许连接不等于所有版本已完成真实服务端验收，认证和 TLS 仍按实际能力校验。
