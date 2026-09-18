# CrashSight 接入设计与实施

## 方案选择

| 方案 | 优点 | 局限 |
| --- | --- | --- |
| 所有事件作为 CrashSight 自定义异常 | 接入简单 | 普通点击混入错误统计，不适合统计启动/时长 |
| 原生 SDK + 独立业务队列（采用） | 崩溃与正常使用统计分开，离线保留、可独立关闭 | 使用统计上传需要自有接收服务 |
| 自建崩溃和埋点服务 | 完全自定义 | 原生捕获、符号解析和运维成本大 |

## 官方接口依据

- [Windows 接入](https://crashsight.wetest.net/documents/zh/crashsight/sdkDocuments/pc-sdk/)：Windows 2.2.1+，加载 CrashSight64.dll；CS_ReportException 是七参数接口，不兼容旧五参数声明。
- [macOS 接入](https://crashsight.wetest.net/documents/zh/crashsight/sdkDocuments/mac-sdk/)：官方原生包为 CrashSight.framework，使用 Objective-C 接口；通过本项目薄桥接层提供稳定 C ABI，Rust 不能直接套用 Windows CS_* 导出。
- [符号文件](https://crashsight.wetest.net/documents/zh/crashsight/symbolsDocuments/symbol-table/)：Windows 保留配套 EXE/PDB，macOS 保留配套 dSYM；符号上传使用控制台生成命令。

用户最终选择海外优先（35c613b255）、国内备用（67d434599d）。区域与 App ID 必须一起切换。客户端初始化只使用 App ID；App Key 不写入客户端或 Git，留给私有 CI/符号上传工具。不要将 CrashSight App Key 当作业务埋点服务的认证令牌。

## 实施顺序

1. 新建独立 diagnostics crate，配置校验、受限事件类型、SQLite 有界队列和后台 HTTP 批量发送。
2. Windows 按官方七参数 ABI 动态加载；macOS 编写使用官方头文件的 Objective-C 桥接库和构建脚本。
3. 主程序启动/正常关闭、设置/导航/对话动作接入。Rust panic 先保存本地简化记录，下次启动通过 SDK 补报；不在 panic/signal 回调中发 HTTP。
4. 保留符号文件，补齐 SDK 分发和安装包入口。SDK 缺失/导出不匹配不得阻止主程序运行。
5. 用本地模拟 DLL、临时 SQLite、HTTP fixture 验证，不向真实项目发送测试崩溃。

## 数据范围

正常事件仅包含固定事件名、随机会话 ID、版本、时间和会话持续时间，不采集 SQL、对话正文、API Key、数据库连接串或账号标识。原生 dump 由官方 SDK 生成，可能包含进程内存和 SDK 自身收集的设备信息；这不等于 dump 已完成脱敏。

已从官方更新记录下载 Windows 2.2.9.1087 SDK，并检查 PE 导出和文件版本；本地路径 `vendor/crashsight/windows/CrashSight64.dll`，该二进制被 Git 忽略。可复现来源和 ZIP SHA256 记录在 `vendor/crashsight/windows-source.json`，此 SHA256 是本次下载计算值，并非官方另行发布的签名。没有初始化真实 SDK 或发送测试报告。

尚未提供原生 macOS framework 和业务统计接收地址；下载包内的 iOS framework 不当作 macOS SDK 使用。macOS 桥接源代码尚需在 macOS 上结合实际 SDK 编译验证。不得将这些待验证部分宣称为已经完成线上上报。

## 启用、路径与降级

内置公开配置为 `crates/desktop/assets/crashsight.json`，程序首次启动时启用本地诊断队列，优先使用海外 App ID。Windows 从 EXE 相邻 `diagnostics/CrashSight64.dll` 加载；macOS 从 `Contents/Frameworks/libturbodbnote_crashsight.dylib` 加载，开发目录也支持 `diagnostics/`。缺少 SDK 时保留本地诊断记录，不会使应用退出。`turbodbn_diagnostics::status()` 明确区分“缺少 SDK”和“SDK 已初始化（未验证投递）”。

可在 EXE 相邻放置 `crashsight.local.json` 覆盖完整配置，该文件已被 Git 忽略。关闭全部诊断：
```json
{"enabled":false}
```
关闭正常使用记录但保留原生崩溃捕获：使用完整配置，将 `record_usage` 设为 false。业务统计未配置接收地址时只写本地，不调用 CrashSight 异常接口伪装普通点击。

诊断目录为 `settings.db` 相邻的 `diagnostics/`：
- `events.db`：正常事件 SQLite 队列，最多 1000 条，单批最多 50 条。内存投递通道最多 128 条，队满时丢弃新事件，避免阻塞 UI。
- `native-<AppID>/`：Windows SDK 的区域独立工作区，不搬运海外 dump 到国内目录。
- `panic-<session>.json`：最多保留约 32 份简化 Rust panic 记录，仅含版本和源行号，不含 panic 文本或路径；下一次启动有 SDK 时补报，改后缀为 `.submitted` 表示提交给 SDK，不代表服务端已接收。

正常退出排队记录 app_stopped 与 elapsed_ms，最多等 4 秒刷新。强制终止时不保证有退出事件；不要将缺失退出事件直接算成原生崩溃。只有原生 SDK 捕获报告才用于崩溃统计。

## 海外失败切换国内：已经实现和待完成的区别

已实现海外/国内目标配置、启动前选择、确认失败标记持久化、区域工作区隔离。`mark_confirmed_primary_failure(config, diagnostics_dir)` 仅供实际 SDK 的已确认上传失败通知或可信投递监控调用，写入 `use-fallback.json` 后，下次启动选择国内。删除这个标记可恢复海外优先。

**尚未实现自动判定海外上传失败。** 当前官方 Windows C API 的 Init/ReportException 返回 void，CrashCallback 只有类型和 GUID，没有成功/失败返回；本次实际 DLL 导出检查也未发现公开文档支持的上传结果 C 接口。不把 DLL 缺失、API Key 缺失、DNS 探测失败或 HTTP 首页状态当作崩溃投递失败；不在同一进程重复 Init，不盲目跨项目重传已有 dump。因此当前默认行为是海外 SDK 自身重试，国内为显式确认后的下次启动备用。要完成自动切换，需要厂商确认实际 SDK 的回调/重初始化/历史 dump 归属契约，或提供可信投递监控。

macOS 官方 SDK 使用自己的缓存，桥接层没有伪造隔离或迁移缓存接口；切换项目后的旧缓存处理也需要实际 SDK 验证。

## 独立统计接收协议

配置 `analytics_endpoint` 为自有 HTTPS 接收地址，可配独立 `analytics_token`，不能填写 CrashSight 的 PC 主机名或 App Key。后台每 30 秒尝试一次，超时 3 秒，不跟随重定向。请求：
```json
{"schema":1,"events":[{"id":"random-event-id","session":"random-session-id","name":"app_started","version":"0.1.0","timestamp_ms":0,"elapsed_ms":0}]}
```
接收方任意 2xx 必须表示**整批已经持久接收**；按 event id 去重。客户端在 2xx 后删除该批队列，其它结果保留并稍后重试。发送成功但响应丢失会重发相同 ID，不保证 exactly-once。当前没有接收服务地址，因此线上业务统计未启用。

## 打包与符号

Windows：
```powershell
pwsh -File scripts/package-windows.ps1 -IsccPath 'C:\Path\ISCC.exe' -CrashSightSdkDirectory 'G:\Path\vendor\crashsight\windows'
```
SDK 来源目录只放可分发运行文件，不放 App Key、符号工具或私有 CI 配置。打包流程检查 DLL 版本并保留独立私有 symbols 目录（匹配的 EXE/PDB），不将 PDB 放入用户安装包。Release 启用调试符号、不 strip。

macOS：
```bash
bash scripts/build-crashsight-macos.sh /path/to/mac-sdk /path/to/App.app/Contents/Frameworks arm64
dsymutil /path/to/App.app/Contents/MacOS/turbodbnote -o /private/symbols/turbodbnote.dSYM
```
验证 bridge/framework/app 的架构及 `otool -L` 依赖路径；按同一 Team 签名并公证，不关闭系统安全检查。正式发布时使用 CrashSight 控制台生成的符号工具命令和区域对应 App ID/Key，符号 ID 必须匹配分发的二进制。此仓库尚无完整 macOS 打包流水线。

## 验证

```powershell
cargo test --locked -p turbodbn-diagnostics
cargo run --locked -p turbodbn-diagnostics --example probe -- target/diagnostics-probe
```
自动化验证使用现场编译的假 DLL 和本机 HTTP fixture，无真实 SDK 初始化、无真实凭据。覆盖初始化顺序、七参数 ABI、缺失 DLL、配置边界、队列上限、失败保留/成功确认以及备用配置标记。


### 本次验证结果
- 工作区 46 项测试通过，其中 diagnostics 8 项覆盖假 DLL ABI、初始化顺序、HTTP 失败/确认和队列边界。
- 本地 probe 验证缺少 SDK 时仍记录启动/动作/正常关闭及会话时长；故意 Rust panic 的独立 probe 返回 101 属预期，持久记录不含测试敏感文本或路径。
- Windows staging 脚本已用官方 2.2.9.1087 DLL 验证复制与版本检查，仅复制未执行。
- 未发送真实 CrashSight 报告、未上传符号、未启用线上业务统计；macOS 仅源码与构建脚本，尚未编译或运行。
- SDK 状态写入“输出”日志，缺失不会伪装成初始化成功。SDK 初始化成功也不代表已完成服务器投递。

- GUI 隔离实例（未带 SDK）启动、关闭成功，SQLite 中包含 app_started/app_stopped。
- 新版 `target/debug/turbodbnote.exe` 已构建，官方 DLL 已复制到 `target/debug/diagnostics/`，未启动这个带真实 SDK 的组合；下一次用户启动会初始化海外项目。可用本地覆盖 `{"enabled":false}` 关闭诊断。
