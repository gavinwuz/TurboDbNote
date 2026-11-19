# Note 内存生命周期验证

诊断构建在可执行文件旁存在 `memory-observation.flag` 时启用计数。无标记时不启动日志线程。

- 统计 NoteView、NotePane、CellView、ScriptView、ScriptResults、TableView 的累计创建、销毁及存活数量。
- 输出同目录 `memory-lifetimes-<PID>.csv`，只含时间、固定类型及计数，不含正文、SQL、连接信息或文件路径。
- 后台队列容量为 1，满时合并通知并累计 coalesced；定时采样保证最终状态可见。最多运行两小时，写入失败输出固定错误并停止记录。
- 每行计数为近似并发快照；判断采用静置后的稳定值，不能要求操作瞬间完全一致。

## 操作

1. 使用 `scripts/build-preview.ps1` 更新固定 `target/preview`，保留 EXE/PDB 配套。
2. 在该目录创建上述标记及内容为 `{"enabled":false}` 的 `crashsight.local.json`，使用便携模式和隔离测试数据；不要复制凭据或自动运行 SQL。
3. 启动后记录 PID，并同步采集进程提交内存、专用工作集、句柄及线程。
4. 固定同批 Note 和单元数量，记录基线；打开后静置 60 秒，全部关闭后静置 60 秒，重复至少 5 轮。
5. 关闭后 NoteView/NotePane/CellView 等应回到最初关闭状态的存活数。ScriptView 和 TableView 须扣除独立打开的表页签及其他合法持有者。
6. 存活数逐轮增长时定位引用或任务持有；存活数恢复而句柄增长时分析句柄类型；两者恢复而内存不平台化时再采集堆快照。

此工具只能证明对象生命周期，不能统计所有堆分配或证明底层组件没有泄漏。不能从 debug 预览版绝对内存量推断 release 的占用。测试结束关闭诊断实例并移除标记；不清理用户文档。
