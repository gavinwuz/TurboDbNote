# 更新日志

发布说明从以下版本章节生成。

## [Unreleased]

- 公开发布 Windows 安装包和独立源码材料 ZIP；绿色 ZIP 排除应用源码目录与 MySQL 源码归档。

## [0.1.0] - 2026-10-12

TurboDBNote 首个公开版本，提供 Windows 与 Linux x64 包。

### 新增

- 原生 SQL Notebook，支持独立 `.note` 文件、恢复、项目分类、最近文件及 600ms 自动保存。
- MySQL/PostgreSQL/SQLite SQL 单元、分页结果网格、结构化执行摘要、影响行数及写入确认。
- Markdown 图标＋提示格式工具与原生撤销；任务清单支持负责人、截止时间、完成状态和本地提醒。
- 16 种数据库驱动，覆盖关系型、分析、NoSQL 与本地文件；外部适配器提供连接、对象浏览及有界只读预览。
- 数据库对象浏览、表定义／检查器、筛选、排序、暂存编辑、可读 INSERT/UPDATE 复制及 JSON 视图。
- 按驱动能力建库、传输／导入／导出，对象选择、尽力权限检查、字符集及 MySQL 排序规则。
- Codex、Claude Code CLI 对话，独立设置／历史、流式输出、停止及明确 SQL 授权。
- 中英文、主题、字体预览、设置持久化及有界结构化操作日志。
- Windows 托盘托管，记住关闭行为，支持打开／退出及待保存保护；Windows 当前用户密码加密与随包 MySQL 兼容组件。
- 自动生成 Windows 便携 ZIP、Linux tar.gz、SHA256 校验及可复用发布准备入口；Windows 安装包与配套符号保留在工作流产物中。

### 平台要求与限制

- Windows 10 1809+／Windows 11 x64；Windows 程序尚未签名。
- Linux x64 包以 Ubuntu 22.04／glibc 2.35 为基线，需正常的 X11/Wayland 会话、Vulkan、fontconfig、libxkbcommon 及 unixODBC；具体依赖见包内清单。
- Linux 密码仅保留在会话内存；Windows DPAPI、托盘、SQL Server 集成认证及随包旧版 MySQL DLL 仅 Windows 提供。
- 外部适配器尚不开放通用 Note SQL、写入或传输；Access/DB2/Oracle 需相应原生客户端。Doris 只读，Redis 集群浏览限当前节点。
- 提醒仅对运行中应用内已打开笔记有效；AI 需另外安装并登录 CLI；不提供自动更新。
- CI 提供隔离协议与窗口启动检查，不等于所有数据库真实服务端、Linux 全桌面／GPU／Wayland 验收；原生崩溃报告投递尚未验证。
