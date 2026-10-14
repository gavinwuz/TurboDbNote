# 原生开发工具与 Python 审计

开发工具统一在工作区包 `scripts/dev-tools`（`turbodbnote-tools`），不依赖 GPUI，不进入用户安装包。所有命令从仓库根目录运行：

```powershell
cargo run --quiet --locked -j 1 -p turbodbnote-tools -- version
cargo run --quiet --locked -j 1 -p turbodbnote-tools -- release --version 0.1.1
cargo run --quiet --locked -j 1 -p turbodbnote-tools -- run-cargo check --locked -j 1 --workspace --all-targets
cargo test --locked -j 1 -p turbodbnote-tools
```

`release --notes FILE` 生成双语说明，`--publication` 检查 UTC+8 正式发布日期。`package-linux` 仅在 Linux 构建原生发行包，检查依赖、生成源码归档和 SHA256，拒绝覆盖已有输出。`build-diagnostics` 通过 `gh` 读取指定工作流任务日志，仅输出有界、脱敏的错误摘要；不读取或输出令牌值。

`prepare-release VERSION` **会修改发布文件、提交并推送默认分支，触发发布流程**，只在明确准备发布时使用；要求干净工作区、默认分支与远端一致、版本递增且标签不存在。当前开发验证不运行此命令。PowerShell `scripts/release.ps1` 调用同一原生入口。

## 已替换并删除

| 原 Python 文件 | Rust 替代 | 实际调用 |
| --- | --- | --- |
| run-cargo.py | run-cargo | Windows 打包、双平台发布工作流 |
| release.py | release / version | 发布版本校验和说明生成 |
| prepare-release.py | prepare-release | release.ps1、发布文档 |
| package-linux.py | package-linux | Linux 发布工作流 |
| read-build-diagnostics.py | build-diagnostics | build-diagnostics.yml |
| test-release.py | 原生单测 | 元数据、锁文件、双语章节、诊断脱敏、PowerShell 校验值兼容，以及临时本地 Git 仓库发布准备回归 |

Cargo.lock 的工作区版本校验从固定包名单改为读取实际成员，新增工具包也必须匹配。Windows 隔离打包源码复制包含该工具包；不读取根目录 config。Rust 工具新增依赖复用锁文件已有版本。

## 保留的 12 个 Python 文件

| 文件 | 用途／调用 | 保留原因 |
| --- | --- | --- |
| verify-windows-icon.py | ci.yml、package-windows.ps1 | 原生 PE 图标资源和多尺寸加载验证 |
| verify-windows-startup.py | release.yml | 独立数据目录启动窗口验收 |
| verify-linux-startup.py | release.yml + Xvfb | Linux 打包后的 X11 窗口验收 |
| verify-window-icons.py | 手动 Windows 窗口图标验证 | 与 PE 静态验证不同，验证实际窗口 |
| verify-auto-connect.py | 手动隔离自动连接回归 | 已有有效交互测试 |
| verify-database-adapters.py | 数据库适配器协议模拟 | 多适配器回归，不等同真实服务端 |
| verify-database-copy.py | 手动隔离数据库复制验收 | 数据和交互验证 |
| verify-database-transfer.py | 手动隔离数据传输验收 | 导入、导出和确认流程验证 |
| verify-database-workbench-ui.py | 手动数据库工作台验收 | 现有窗口操作、表格回归 |
| verify-update-handoff.py | 隔离更新交接模拟 | 假安装器／下载交接，不安装真实发行包 |
| generate-icons.py | 手工再生应用 ICO | resvg / Pillow 资源维护，不在普通构建执行 |
| generate-driver-logos.py | 手工再生数据库品牌图标 | 校验来源和生成资源，不在普通构建执行 |

这些工具不是应用运行时依赖。暂不删除有效验证，也不以同名空壳 Rust 命令替代平台检查。剩余 CI 窗口／图标脚本仍需要 Python；因此不宣称整个 CI 已完全移除 Python。后续迁移需保留各平台真实检查，并分别验收 Windows 和 Linux。

本轮已在 Windows 验证 Rust 工具单测、编译和本地版本校验；未在 Linux 执行打包，发布准备回归仅提交／推送临时本地 Git 仓库，未对项目仓库执行发布准备、推送、触发远端工作流或发布。
