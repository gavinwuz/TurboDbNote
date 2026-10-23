# Note 工作台

## SQL查询、SQL文件与紧凑结果

顶部新增菜单和底部快捷栏统一提供 SQL查询／SQL文件／MD／任务。SQL查询合并原 SQL 与可编辑脚本，统一执行按钮：有有效完整选区则执行选区，否则执行本单元全部 SQL；Note 全局全部执行仍冻结多个单元的顺序与范围。单条常规查询保留现有可靠单表编辑能力，多条复用冻结批次与独立只读结果分页。查看计划仍限定单条，未选中时使用光标所在语句。删除重复的执行全部按钮及转脚本／拆分入口。

两类单元共用单行结果状态栏与结果页签；单结果隐藏页签，可靠单表结果以表名标识，同名追加序号，其他使用结果编号。详细统计、SQL 正文、执行明细及表结构按需展开；只读说明以图标提示，不占重复行。纯查询直接执行，风险操作仍冻结目标并明确授权，失败／取消停止后续语句，连接代次与结构校验保持有效。

SQL文件导入外部 .sql 的受管副本，仅创建附件单元，不执行；原文件变化不会自动同步。新增 SQL文件与全局“导入 SQL文件”复用同一选择、方言和预览流程，取消不创建空单元。更换文件保留单元 ID、标题、连接和数据库，清除旧报告与预览；运行中不能更换。正文预览默认收起，执行结果使用相同紧凑布局。旧 sql／sql_script_text 标签继续兼容并统一显示为 SQL查询，旧 sql_script 附件显示为 SQL文件，不重复复制已有资源，也不在恢复时执行。

查询批次正文最多 1 MiB，辅助分析仍为 60 KB；SQL文件仍最多 512 MiB／100 万条、单语句 1 MiB。.note 保持 v4 并兼容读取 v1–v3，文档不保存运行结果和授权。

本次 Windows 隔离 SQLite 窗口验证包括：旧 sql 与 sql_script_text 单元统一执行、整单元三结果／完整单语句选区、同表名序号、独立分页与详情收放后保留页码；新增 SQL文件取消不创建单元、导入不执行、原文件修改不影响副本、执行后的紧凑结果、更换时保留 ID／标题／连接范围且不执行新内容。另验证单条写入在预览与第二次确认前均未改变隔离数据，明确确认后显示单行摘要并可展开明细。Debug 与 Release 成品均完成窗口回归。服务与工作区测试 373 项通过、1 项忽略；不代表真实 MySQL／PostgreSQL 或 Linux GUI 已验收。

T 在光标处仅列出当前连接、当前库缓存中匹配的前八个表名及注释。Col 与点号补全显示列名、类型及注释；注释缺失时不额外请求远程元数据，接受补全只插入标识符。手动 Col 继续提供选表、加载字段和重试入口。

辅助通过专用候选请求入口触发，不制造空文本修改、不改变撤销历史。连续三次自动辅助任务耗时达到 80ms 后暂停自动候选五秒，手动 T／Col／Ctrl+Space 仍可用。单元更多菜单可临时暂停自动高亮与自动补全，正文编辑、光标、选区与保存保持工作；该暂停不写入文档或全局设置。

Windows 隔离 SQLite 窗口已验证：顶部与底部新增脚本、F5 单次授权返回两个结果页签、分页独立保存、T 从超过 6200 项的缓存目录按前缀匹配并显示注释、点号显式加载后插入带引用列名、37,515 字节 SQL 连续输入与中文替换后一次撤销、底部回顶。缓存表注释采用自建目录快照；列类型由隔离 SQLite 返回，列注释组合由单测验证，不代表真实 MySQL／PostgreSQL 服务端或真实 IME 已验收。

## 增量编辑与布局

SQL 编辑复用有界文本片段排版缓存，正文变化只使文本或样式已变的片段失效。自动换行长行仅排版视口附近及光标／选区端点的折行，其余保留完整字节与行高几何；原始正文、复制、保存与执行不截断。行顶与折行字节索引用于光标定位，未显示区域的几何不依赖完整排版。字体、字号、样式与窗口宽度变化仍按相应上下文重新计算。

输入期间保留未受影响的高亮，新增／替换部分暂用普通样式，后台完成后按当前修订发布。待映射编辑有界，主题或方言变化废弃不兼容样式；IME 组词期间不发布新分析。光标闪烁共用等待任务，仅可见状态变化才通知；空补全清理和未变化的滚动不额外通知。

Note 高度达到显示上限后不再随全文实际高度反复刷新外层；结果仅在首次变旧时更新状态。600ms 自动保存共用一个等待任务，界面线程冻结不可变正文快照，后台完成正文字符串构造与文件写入；保存失败、关闭刷新与并发修订保护仍保留。

Windows 隔离 SQLite 回归已检查：短 SQL、24KB／37KB 长行、5000 行文档的光标定位与正文保存，跨行替换、中文文本替换及一次撤销，自动补全、T／Col、脚本多结果与底部回顶。369 项工作区测试通过、1 项忽略。同步消息耗时在本机 Debug 对照中未呈现稳定显著下降；PrintWindow 图像观察包含采集开销，不能作为真实按键到呈现延迟或“原生响应”保证，真实 IME 与 Linux GUI 未验收。

同机隔离 Release 消息探针（每项 133 个输入字符／160 次左右移动）：短 SQL 字符处理 P95 约 1.63ms，24KB SQL 约 1.62ms，左右键约 6.42ms；构建停止后的 Debug 复测分别约 10.46ms（短 SQL）与 56.85ms（左右键）。这同时包含构建优化的收益，不能据此推导方案 B 单独带来的增幅，更不等同按键到屏幕显示延迟。自动化环境的前台焦点校验未通过，因此不使用这些数据宣称前台原生输入已经验收。

Release 对照复用 `scripts/prepare-package-build.ps1` 的固定隔离源码与 FXC 显式参数；命令使用 `cargo build --release --locked -j 1 --config 'profile.release.package.gpui-component-assets.opt-level=1' --target-dir <仓库>/target -p turbodbnote --bins`。仅图标资源包降低优化级别以控制首次构建耗时，应用、GPUI 与编辑器继续使用原 Release 优化；未修改项目默认构建配置或全局 Cargo 源码。该产物为本机构建，不是已发布安装包。


## SQL 输入响应

SQL 单元正文修改仍立即更新执行修订和结果有效性，但已有结果只在首次过期时同步网格，后续输入不反复重建结果快照。编辑器高度、空白状态或结果有效性变化时才刷新单元外层，普通字母输入由编辑器自身更新；SQL 空白状态变化仍及时更新笔记执行按钮。普通 Note 使用可变高度、焦点感知的虚拟列表，首次测量各单元高度后仅布局可见单元及离屏编辑器；保留所有单元实体、编辑器、草稿、结果和会话。单元状态通知使对应高度重新测量，窗口宽度变化由列表重新测量，结构／字体变化更新高度缓存；独立表查询页仍使用完整高度。笔记只在首次变脏或清除保存错误时刷新页签与索引提示；结构、名称、项目等操作保留原有通知。自动保存仍在停止编辑 600ms 后执行，关闭前刷新待保存内容。

普通输入在组件补全菜单创建前拦截，连续输入共用一个 50ms 空闲等待任务；停顿后才进入后台补全。Ctrl+Space 立即调度，列元数据就绪仅在未持续输入、未取消补全时刷新。全文分析、候选生成及编辑范围构造在后台执行；复用当前连接和数据库的有界元数据缓存，不扫描其它连接，不因补全执行 SQL。候选返回前核对请求序号、正文、光标、连接、数据库及连接代次；离开编辑态、切走笔记或输入法组词期间不显示候选。保留原有 60 KB SQL 辅助上限，不截断原文。

长单行保护：任意物理行超过 8192 个 UTF-8 字节时，SQL 编辑与阅读采用轻量高亮，避免同一排版行累积大量样式分段。编辑态显示简短提示；只调整着色，完整正文、行号、复制、补全、保存和执行仍保持原有行为。拆分或编辑使所有行回到上限以内后自动恢复对应驱动的完整高亮，不增加设置项、不改写 SQL。它针对长单行，不代表大型笔记的全部排版开销已解决。

持续输入验证在 31 单元笔记内按每秒 5／10／20 字符各回放 133 个字符，顺序和全文均完整，未编辑单元内容保持一致；最后一个字符发出后约 0.5–0.8 秒内观察到完整文件保存。该结果包含 600ms 空闲保存策略，不是逐帧显示延迟测量；真实输入法组词和不同设备仍需单独验证。

Windows 10 本机 Debug 隔离窗口验证：对含 5000 个数值的 23,929 字节单行 SQL 逐字输入 133 个字符，WM_CHAR 消息处理 P95 从约 541ms 降至约 53ms，全文自动保存校验通过；这不是按键到屏幕呈现的延迟测量。31 单元笔记经过前置防抖和可见区域布局后，同类测量从约 91ms 降至约 31ms；这不代表所有文档、字体和 GPU 场景的延迟保证。另验证轻量模式清空后恢复完整高亮与补全、空白 SQL 的执行按钮更新、多行输入和立即关闭保存；T／Col 的中文带引号标识符、选区替换、一次撤销、Esc 返回、手动补全、草稿不执行及结构／数据导航使用隔离 SQLite 验证，虚拟列表从底部返回后能继续原 SQL 编辑且未修改离屏单元；验证只使用专用目录与隔离 SQLite，不代表真实 MySQL/PostgreSQL 服务端、Linux GUI 或真实输入法组词已验收。

## T 表名与 Col 列名入口

SQL 编辑工具栏在 Col 前提供 T 图标，直接在光标处打开当前连接与库的缓存前缀候选，展示表名及注释，最多八项；继续输入会收窄结果。方向键选择，Enter／Tab 插入，Esc 关闭候选。插入只修改原光标／选区，按驱动引用标识符并支持撤销；不执行 SQL、不扫描其它连接。

Col 使用相同目录入口先选表，再搜索并插入列名，无需先输入 `u.`。列选择是明确的手动选择，不推断 SQL 含义，不自动添加别名、JOIN 或 FROM；多表同名列由用户补齐限定。自动别名点号补全仍只映射可确定的原表，不推断派生表和 CTE。列缓存只保留最近一张表，最多 4096 列；表目录使用已有分页能力与 5000 项显示上限，达到上限提示收窄搜索。搜索输入可清空，提供加载、空态、刷新和重试。

弹层绑定单元修订、连接 ID、当前库和连接代次；编辑或连接范围变化使旧选择失效。复用弹层内目录缓存，不随 SQL 逐字刷新，不扫描其它连接。尚未连接时提示先连接；Redis／当前未接入的外部驱动说明使用数据库对象浏览器，不伪装 SQL 表名列表。自动保存仍遵循 600ms。

## SQL文件导入

Note 顶部工具栏提供导入入口。选择一个 `.sql` 文件后，按当前默认连接的 MySQL / PostgreSQL / SQLite 方言解析；未指定有效连接时明确选择方言。预览显示语句数量、读写分类、不支持项和首批摘要，点击“导入”只创建单元，不执行数据库操作。

创建一个 SQL文件单元，正文和固定长度偏移索引保存到同目录 `.turbodbnote-scripts`，`.note` v4 只保存校验值、统计和连接引用，兼容读取 v1/v2/v3。正文清单按需展开、虚拟分页，每页 6 项、每项最多 512 字节摘要；仅按需读取选中语句，不建立成千上万的编辑器或结果实体。导入不再提供拆分选项，笔记仍受 128 单元／16 MiB 限制。

文件须为 UTF-8（可带 BOM），最多 512 MiB／100 万条；脚本单语句最多 1 MiB，常规单语句通路仍为 60 KB，更大查询经冻结批次通路执行。解析保留原始字符串、注释和标识符；MySQL 转义字符串按兼容 SQL 模式处理，含糊转义、不完整文本和不支持指令均明确拒绝。含不支持项时禁止整份执行；仅无会话依赖、目标核对通过的普通语句支持单条执行。不是完整 SQL dump 恢复工具。

执行整份或选中项前冻结资源并校验源文件／索引 SHA256、目标连接和权限。一个脚本运行复用一个专属数据库会话，保持原 SQL，不为了预览改写 LIMIT。开始前以一个窗口预览整份脚本、目标、分类和风险清单，明确点击“授权并执行”后开始；单次批量授权仅对本次冻结内容生效，预览不执行正文。记录确认时的结构指纹，实际执行前逐条准备并再次核对连接代次、许可和结构。脚本自身成功写入后更新结构指纹，允许建表／改表后继续执行；外部结构变化停止执行。结构指纹仅保存哈希，最多 4096 个对象。MySQL 中未知函数／UDF 因可能有副作用也要求预授权，不能利用普通 SELECT 绕过确认。关闭确认、失败、取消或配置失效停止后续语句；停止不撤销已提交写入，取消时正在发送的语句可能已经生效，不自动重试。MySQL 专属会话结束时先尝试 ROLLBACK，成功后才尝试 UNLOCK TABLES，随后关闭连接；不会盲目执行脚本剩余尾部。恢复笔记不会执行脚本。

查询行集按语句保留有界独立结果页签，默认每页 20 行；可靠可重查的无会话依赖查询支持独立分页，会话依赖结果明确冻结为当前页。最多保留 50 个行集并限制字节总量，超限明确提示淘汰较早结果。底层请求仍有响应行数／字节上限，不能将分页理解为无界流式读取；批次结果不提供修改提交。

另存为、应用内改名和归档携带附件；手动移动／备份 `.note` 必须同时携带 `.turbodbnote-scripts`，只有 `.note` 不再代表脚本正文完整副本。资源内容按哈希共享，删除单元不自动删除附件，避免破坏其它笔记引用；目前未提供垃圾回收或附件编辑功能。

服务层回归覆盖 UTF-8／BOM、注释和引用边界、不支持项、取消、篡改、资源搬移、10 万条磁盘分页，以及同一隔离 SQLite 会话的逐条准备／确认／执行和结构变化拦截。该验证不代表真实 MySQL／PostgreSQL 服务端或 Linux GUI 已验收。 Windows 原生隔离窗口另已验证顶部导入、导入不执行、逐条写入双重确认与取消、1000 条虚拟分页／第 51 条执行／完整执行、笔记全部执行失败停止，以及最终 EXE 重启恢复不执行。

## 方案与选择

| 方案 | 特点 | 取舍 |
| --- | --- | --- |
| A：独立 `.note` + 笔记索引（采用） | 每份文档可独立备份；项目与连接侧共用；索引记录最近和打开页签 | 需处理外部修改与目录变更 |
| B：所有笔记集中 SQLite | 索引和正文可统一事务 | 单文件导出、直接备份及外部打开成本更高 |
| C：纯文件夹树 | 路径直观 | 项目分类依赖目录，移动路径影响使用记录 |

## MySQL 导出脚本的受控上下文

支持单库 USE、UTF-8 系列 SET NAMES / SET CHARACTER SET、会话级外键／唯一性检查、SQL_MODE / TIME_ZONE / SQL_NOTES / autocommit，以及已知 @OLD_ 和 mysqldump @saved_cs_client 等变量保存／恢复模板。表锁支持 LOCK TABLES 表 READ/WRITE 与 UNLOCK TABLES；事务支持 BEGIN / START TRANSACTION / COMMIT / ROLLBACK。识别 MySQL /*!版本号 … */，按实际服务器版本执行内容，不作为普通注释跳过。

普通 SQL 单元和 AI 工具仍不开放这些控制指令；设置 → 数据库 → SQL 中有独立脚本许可。会话设置和事务按默认许可提供，表锁默认关闭；已有明确命令白名单不自动添加新许可。整个脚本以一次批量授权覆盖本次冻结的风险清单，持锁期间不等待弹窗、不自动添加只读事务，也不在每次操作前后重置字符集。结构读取只访问需要核对的目标，所有命令保留同一物理会话。

USE 或跨库对象与当前目标不一致时，在正文执行前阻止并打开连接／数据库选择框。选择后关闭并重新执行，不自动切库、改写 SQL、连接其它服务器或自动执行。单条执行仍核对整文件 USE；含会话设置、锁或事务的脚本须完整执行，单条入口不自动执行前置设置。锁定不成对、锁定期间重新开始事务、未结束事务或未明确提交的写入会在预检查阶段拒绝。事务不能保证 DDL 和表锁相关操作可回滚，已经提交的结果不会撤销；恢复外键检查不会自动补查此前数据。

不支持 SET GLOBAL、GTID／复制／binlog 管理、全局锁、文件／客户端命令、DELIMITER、存储程序或多库恢复。仅开放安全字面量及已知保存恢复模板，不开放任意 SET 表达式。需要兼容的初始 SQL_MODE；ANSI_QUOTES / NO_BACKSLASH_ESCAPES 会明确拒绝，避免将 SQL 按错误语义预授权。高级会话控制以现代原生 MySQL 为验收范围；其它驱动沿用既有普通 SQL 能力。

禁用“执行脚本”按钮的外层始终可悬停，说明正在运行、批量锁定、索引升级或不支持语句数量；警告图标显示具体原因。旧 TDBSQL01 索引后台自动重新分类并发布新的派生索引，保留原 SQL、原索引和单元 ID，更新引用后保存；不需要重新导入，不触发 SQL。

Windows 上的独立 MySQL 8.0.32 已验证上述头尾控制、中文／转义字符串、超过 60 KB 的批量 INSERT、逐条确认拒绝、字符集跨请求保留及关闭会话后表锁释放。测试使用新初始化的 loopback-only 服务器和自有数据目录，没有连接或修改用户数据库。该验收不代表其它 MySQL 版本、MariaDB、PostgreSQL 或 Linux 窗口已验收。

## 使用

- 左侧笔记标题栏提供新建、打开与最近 5 项；筛选框支持清空。列表按数据库侧共用的项目分类。
- 新建立即生成独立 UUID 命名的 `.note` 草稿，未命名也有磁盘文件。标题可稍后填写；另存为用于选择正式文件名。
- 每份文档可添加、复制、前后插入、移动与删除 SQL、Markdown 和任务子区域。删除需要确认；阅读/编辑切换和切页保留编辑及执行状态。
- SQL 子区域选择已保存的 MySQL、PostgreSQL 或 SQLite 连接；数据库留空时沿用连接默认库。Markdown 预览和任务状态不依赖连接。
- SQL 提供执行、停止以及与“数据库 → 查看数据”共用的结果表格、复制和分页，默认每页 20 行，可设 1–1000 行。Ctrl+Enter 执行，可在数据库设置改成 Enter；Shift+Enter 保留换行。Ctrl+S 保存笔记。

## 临时目录与自动保存

“设置 → 通用 → 笔记存储”可选择临时目录或恢复默认；这些设置沿用 connections.db 中 settings 文档的 600ms 自动保存机制。

默认目录是应用数据目录下 `saved`。便携版使用可执行文件旁的 `data/saved`。更换目录仅影响之后新建的草稿，已有笔记继续保存到原路径；索引保留旧目录和文档路径。另存为成功后，仅移除属于该文档且没有外部修改的原 UUID 草稿。

笔记自动保存默认开启，停止编辑 600ms 后写盘。关闭后可手动保存，关闭未保存笔记时提示“继续编辑 / 保存并关闭”。保存失败保留编辑内容和页签，不自动关闭。另存为不覆盖现有文件；原文件被外部修改时阻止覆盖，并提示另存为或重新打开。

文件先写同目录临时文件、刷新后替换；索引异步合并写入，不保存查询结果和连接凭据。文档中只有连接 ID 与库名引用。笔记正文按用户内容原样保存，不能把它当作凭据保险箱。

## SQL 执行边界

- SQL查询支持完整选区或整单元的多语句执行；会话控制依驱动与脚本命令许可，不能跨批次依赖临时会话。可编辑批次正文上限 1 MiB，文档 16 MiB、128 个子区域，同时最多打开 16 份文档，索引最多 2000 项。
- 每次请求使用独立后台数据库会话，停止不会断开数据库管理页会话。切换连接、修改代码或连接配置会使旧结果/预览失效；重启只恢复文档，不自动执行 SQL。
- 常规分页保留用户原始 SQL 字面量，只调整最外层数字 LIMIT/OFFSET。默认多读一行判断下一页，不执行 COUNT。大偏移查询仍有性能成本；需要稳定结果时指定 ORDER BY。
- INSERT/UPDATE/DELETE/CREATE TABLE/ALTER TABLE/DROP TABLE/TRUNCATE 先生成预览，再明确确认，执行前复核连接快照和目标结构。预览后改变 SQL 必须重新生成。查询不能包含写入 CTE、SELECT INTO 或带锁子句。
- SQLite 与现代 MySQL/PostgreSQL 查询使用数据库只读保护；旧 MySQL 不提供同等只读事务保证，因此查询也需要确认。SQL 无法保证跨语句的一致快照，取消或网络错误后的写入可能已生效，不自动重试。
- 结果复用有界驱动读取与虚拟表格，最多 512 列、约 768 KiB 响应；超限报错，建议减少每页行数/列数。显示预览截短不会改写原始复制值。
- 后台 SQL 与笔记保存使用底部输出的有界结构化日志，不记录正文、密码或原始服务端错误。

## 验证范围

服务测试覆盖笔记独立保存/恢复、外部修改冲突、目录切换、未知版本保护，以及 SQL 分页字面量保留、写入确认和结构变化拦截。Windows 原生窗口使用隔离 SQLite 检查保存、执行、分页、Markdown/任务、设置路径选择、自动保存关闭提示、另存为、重启恢复及关闭流程。SQLite 写入预览前不执行，二次确认后写入成功；检查磁盘日志未包含测试 SQL 正文。MySQL/PostgreSQL 的本次 Note 通路没有真实服务器验收；不将已有连接管理验证等同于 Note 查询验收。

## 单元阅读与编辑布局

恢复和打开时以阅读态展示正文、注释与已有运行期结果；双击正文或按 F2 进入单元编辑，Esc 或眼睛图标返回阅读态。每份 Note 同时只有一个单元展开编辑栏，左侧主题色条及边框标识当前编辑单元；这只切换界面，不修改数据库连接的只读权限。

编辑栏左侧为返回阅读、注释/标记（兼容已有 cell.title）、连接和数据库。数据库支持手填及可搜索下拉；复用现有目录，并可明确点击读取按钮获取服务器列表，不静默连接所有服务器。SQLite 显示当前文件。右侧使用图标及悬停提示：执行/停止、复制单元、上移、下移、前插、后插、删除。窄窗口允许工具栏换行。

复制仅复制类型、注释、正文和连接引用，新 ID 的副本紧接当前单元；不复制或重放查询结果。前后插入使用当前类型与 SQL 绑定，正文为空。移动保留原实体和查询状态；首尾禁用无效移动，达到文档上限时禁用新增。删除需要确认，包含未提交结果修改的撤销提示。

SQL/Markdown/任务新增入口位于滚动内容的最底部，仅在该区域悬停或键盘聚焦时显示图标；空文档保持可见。多行内容和所有文本输入保留非空清空入口。

## SQL 结果与表数据页对齐

SELECT 结果复用 TableView 的双向虚拟网格、字段类型图标、行号、列菜单、行列高亮、检查器、复制和导出，以及底部单行页大小/首页/上一页/页码/下一页/尾页/手动总数工具栏。结果区嵌入当前单元；未提交的数据修改不写入 .note 文件。

- 直接映射单个原表、列名唯一且包含完整可靠主键的查询可暂存修改，经 SQL 预览和二次确认后事务提交。支持普通列投影（如 SELECT id,name FROM items），按投影列顺序校验原值，未选择的字段不被更新。投影缺少表的全部列时不允许删除原行，避免忽略未展示字段的并发变化。
- 单表普通列查询可以添加筛选和排序，保留原始 WHERE/ORDER BY/LIMIT 语义，附加条件作用于原查询结果；清除界面筛选不会删除原 SQL 条件。原排序表达式无法映射到输出列时，提示先选择结果列排序。
- JOIN、自连接、UNION、CTE、聚合、DISTINCT、计算表达式、重名列或重命名列结果保留只读浏览。缺少主键的单表结果仍可筛选，但不能修改原行。无可靠原表时不生成伪造目标表的 INSERT 复制文本。
- SQL 或数据库输入变化后，旧结果标为失效；连接快照和表结构在提交时再次核对。存在未提交表格修改时阻止重跑；关闭 Note 需确认停止任务、撤销未提交表格修改并保存笔记。已提交的数据库修改不能通过关闭 Note 撤销。
- 任意查询结果采用偏移分页，每页默认 20 行；不会自动统计总数。独立表数据页的主键游标策略保持原样。旧 MySQL 查询结果暂不开放写入，4.0 不支持派生结果筛选/统计。

相关回归覆盖投影列顺序、筛选与计数保持原查询范围、未确认提交拒绝、并发值/DDL 变化拒绝，以及 JOIN/聚合/缺主键等只读边界。数据库列目录与结果请求使用独立后台会话，旧连接结果不得回填当前单元。

## 顶部工具条、默认范围与文件操作

采用紧凑工具条 + 页签元信息：相比将全部功能隐藏在菜单（入口更深），以及可折叠元信息栏（仍重复占用空间），此方案直接保留常用操作，将文件设置按需展开。

顶部仅显示新增单元菜单、保存、按顺序执行全部 SQL、全部终止、全部折叠、全部展开，以及默认连接/数据库。新增类型由同一类型目录提供给顶部菜单与底部插入入口，目前为 SQL、Markdown、任务；未实现的类型不显示为可执行能力。

没有指定连接的 SQL 单元继承 Note 默认连接；数据库留空时继承对应默认库。选择自身连接后保持独立，单元菜单可恢复“使用笔记默认配置”；数据库也可单独覆盖。默认设置随 .note 保存，旧文件缺少这些字段时保持未指定，不覆盖已有明确的单元配置。新建和未指定连接的单元跟随默认范围；更改默认范围会使这些单元的旧结果失效。执行中或有暂存结果修改时不允许更改默认范围。

名称、项目和保存文字不再常驻内容区。页签使用红点（未保存/失败）、黄点（保存中）、绿点（已保存），并在悬停提示中用文字给出状态、项目和文件位置。页签与侧栏 Note 文件行的右键菜单提供重命名、另存为、项目、打开所在文件夹；其它类型页签保持原菜单。另存为使用系统文件位置窗口。未命名草稿的重命名修改显示名称；已命名文件在同目录生成新名称并仅在原文件未被外部修改时移除原文件，不覆盖已有目标。

全部执行在开始时冻结顺序和连接快照，跳过 Markdown、任务及空 SQL；逐个执行，普通 SQL 单元的风险语句仍逐项预览／确认；SQL 脚本单元按本次冻结脚本一次批量授权。失败、取消预览或终止后不执行余下单元。它不是跨单元事务，停止不会撤销已完成写入，也不会丢弃结果表格中未提交的修改。折叠只隐藏界面，不结束查询或丢弃编辑器/结果状态。

为容纳原生 Notebook / 表格 / 对话框的嵌套调试帧，Windows 主程序保留 8 MiB 栈地址空间，并拆分工具栏与网格渲染；不修改系统线程设置或第三方依赖源码。

最终 Windows 隔离窗口验证了单表结果暂存/确认提交、筛选与手动计数、关联结果只读分页、单元复制不重复执行、移动与前后插入、文件重命名/项目/另存为、批量逐项确认与取消、等待连接时全部终止、默认范围继承及独立覆盖。默认范围切换后旧结果失效；同一查询在两个隔离 SQLite 文件中返回了各自的结果。MySQL 网络停止使用本机模拟等待端点，不等同于真实 MySQL 查询验收。


## 布局和存储选择

SQL / Markdown 新增操作共用 SQL / MD 字母轮廓图标。注释框缩短至 160px，阅读按钮置于其前；数据库输入采用 260px（单元）/ 280px（Note 默认），搜索下拉独立为 360px，窄窗口允许换行。页签提示限制宽度，长路径省略中段；实际路径不变，可从文件右键打开所在文件夹。

笔记项目标题右侧提供拖动柄，排序与数据库侧栏共用持久化项目顺序，未指定项目不提供排序柄。文件行相对项目标题缩进。笔记入口页复用同一文件库，提供新建、打开和最多五个最近使用文件（发现但未打开的文件不伪造使用记录）。

| 存储方案 | 优点 | 代价与适用范围 |
| --- | --- | --- |
| 独立 JSON（采用） | 单文件可搬移、可人工恢复、易比较；兼容已有文件 | 修改后整份保存，适合当前有界文本笔记 |
| 压缩 JSON | 重复文本占用较小 | 每次解压/重压，无法直接编辑，需要解压大小限制 |
| SQLite 单文件 | 单元增量更新、事务和索引 | 文档格式与迁移更复杂，不便文本比较，适合未来超大笔记 |

参考 [Jupyter nbformat](https://nbformat.readthedocs.io/en/latest/format_description.html) 的 JSON、版本和单元组织方式；并非 .ipynb 格式兼容。Jupyter 可保存执行输出，TurboDBNote 当前只保存源码、注释及连接 ID 引用，不嵌入查询结果或连接密码。用户自己写入正文的敏感内容仍是普通明文，应自行避免保存；JSON/压缩/SQLite 本身均不等于加密。

解析先用轻量版本结构跳过正文检查版本，再直接解析模型，移除完整 JSON Value 树及其克隆；大文本仅保留输入缓冲与最终模型，不宣称一次扫描。继续保留易读缩进格式、16 MiB 文件上限、128 单元限制、600ms 防抖、后台读写、临时文件同步后原子替换、外部修改哈希校验、拒绝覆盖新文件与未来格式。打开文件不会自动执行 SQL。大文档仍是整份读写，不宣称增量存储或加密。

本轮 Windows 隔离窗口已验证默认库/单元库的搜索弹层与清空入口、阅读按钮位置、文件缩进、项目拖动后持久化并同步数据库侧栏、入口最近文件打开与新建，以及长路径提示不溢出。全量测试复跑通过；首轮既有 Windows 凭据迁移测试出现一次失败，单独与全量复测均通过。未执行真实网络数据库查询，本轮不扩展服务器验收范围。

图标采用中性方框与右上角彩色字形叠加：SQL 蓝色、MD 绿色、Note 紫色 N；页签、文件列表、最近菜单与入口统一复用，保存状态圆点独立。


## SQL 输入辅助

采用本地方言词库 + 编辑器原生补全弹层 + 已加载表目录，避免引入外部语言服务或将输入发送给 AI。与语言服务器方案相比无需安装维护进程，但不提供完整字段/别名解析；AI 方案适合单独的复杂查询生成，不作为基础输入提示。

- 单元自身连接优先，否则继承 Note 默认连接；未指定时使用通用 SQL。MySQL、SQLite、PostgreSQL、ClickHouse 各自提供关键词和常用函数签名，Redis 只提供命令及参数格式。函数词库为常用子集，不代表服务器全部函数/插件或所有历史版本均可用；版本相关项附带版本提示。
- 输入单词自动出现候选，方向键选择，Enter/Tab 接受，Esc 先关闭候选，再次 Esc 才进入阅读态。即使设置 Enter 执行 SQL，补全确认也不会执行查询。
- 语句开头输入表名，选择“生成 SELECT 查询”可生成带方言引用的 `SELECT * FROM table;`。FROM/JOIN 等位置只插入表名；已存在的尾分号不会重复插入。替换范围仅为光标所在单词，保留其他内容。目录来自当前已连接且已加载的对应库，最多 2000 个对象；无目录时仍可根据输入的简单表名生成模板，不表示该表已确认存在。不会为补全自动连接数据库。
- 常用函数候选展示参数签名，选择后插入函数括号；未提供全量语义检查或参数类型推断。字符串、未闭合引号和注释内不提示。中文和光标位于单词中间的替换按字符边界处理。
- 编辑栏“格式化 SQL”或 Ctrl+Shift+F 对整份单元 SQL 格式化，默认软行宽 100 字符、2 空格缩进；长字符串、标识符、注释不强拆。保留原始 token 文本与大小写，格式化前后同时校验 token 与 AST 展示结果。解析不支持、未闭合文本及 MySQL 执行注释保持原文并提示；Redis 不作 SQL 格式化。
- 格式化在后台执行，应用前验证文本、修订和连接/库上下文，格式化可撤销；运行中或结果有暂存修改时不应用。补全和格式化限制 60 KB 源码，常规候选最多 60 项，自动列提示仅展示前八项（底层列候选能力保留）；不影响更大笔记的保存。
- ClickHouse 与 Redis 可以选择为编辑方言，但当前 Note 查询执行仍仅支持 MySQL、PostgreSQL、SQLite；界面显示编辑能力提示，单元执行按钮禁用，批量执行仍预检拒绝不支持的驱动。

词库参考官方文档：[MySQL](https://dev.mysql.com/doc/refman/8.4/en/functions.html)、[SQLite](https://www.sqlite.org/lang_corefunc.html)、[PostgreSQL](https://www.postgresql.org/docs/current/functions.html)、[ClickHouse](https://clickhouse.com/docs/sql-reference/functions)、[Redis](https://redis.io/docs/latest/commands/)。仅在开发时参考，运行期补全不访问这些网站。

本轮验证：8 项 SQL 辅助回归测试覆盖方言、替换范围、schema、转义、注释、换行敏感字符串、嵌套查询与格式化幂等。Windows 隔离窗口验证了 Enter/Tab 接受候选不执行、再次 Enter 按设置执行隔离 SQLite 查询、MySQL/PG/ClickHouse 函数签名、Redis 命令及禁用执行、已加载 SQLite 目录补全、格式化按钮/快捷键及 Ctrl+Z 撤销。完整工作区测试串行复跑通过；首轮既有 Windows 凭据迁移测试偶发失败，复跑通过。未连接真实网络数据库进行本轮验收。


## 查询执行与资源管理

当前 SQL 单元支持 F5 执行，编辑器内保留 Ctrl+Enter / 设置中的 Enter 执行；补全确认优先。执行按钮之后为执行计划（Ctrl+E），只对单条查询发送 EXPLAIN，SQLite 使用 EXPLAIN QUERY PLAN，不使用 EXPLAIN ANALYZE，不允许借此执行 DML。

旧 MySQL 缺少只读事务时，普通 SELECT 和常见无写入副作用的内置函数不再一律要求确认；未知存储函数/UDF 调用仍确认。真正写入继续冻结 SQL 与目标、二次确认并核对结构，不能通过 SELECT INTO 文件或锁定查询绕过。

常驻内存主要包括 GPUI 字体/图像/布局缓存、Note 编辑器与撤销历史、当前页原始结果及有界显示缓存、连接会话及有界输出日志。Codex/Claude 子进程有独立内存，任务管理器应用组汇总不能等同于主进程泄漏；工作集与私有提交也不是同一指标。不能以清空 Windows 工作集代替修复资源生命周期。

结果交接采用移动而非整页克隆；接收普通结果后释放旧的备用计划网格。无结果列时避免创建可见虚拟表格。单元提供“清除结果”，释放结果实体、分页状态及显示缓存，保留 SQL；运行中或存在暂存修改时不允许清除。折叠与切页仍保留原有实体和草稿。

本轮资源采样使用 Windows debug、隔离 SQLite、两个 Note 单元及关闭 AI 自动连接：旧版启动工作集约 86.6 MiB/私有内存约 100.1 MiB，查询后工作集约 94.6 MiB/私有约 102.7 MiB，连续采样稳定；优化后对应约 94.5/102.3 MiB，清除结果后约 92.6/101 MiB。约 5 秒采样间隔内，结果页 CPU 时间从约 1.7–1.8 秒降到 1.3–1.4 秒，清除后约 0.7 秒。该短时隔离样本不能证明真实数据、网络驱动或长时间会话没有泄漏，也不能与任务管理器应用组百分比直接比较。原生窗口验证 F5、执行计划和结果清除；普通旧版 MySQL SELECT 通过适配器模拟验证，本轮未连接真实 MySQL 服务器。


## 表别名字段补全与网格高亮

输入 `SELECT * FROM users u WHERE u.` 会按作用域解析别名并提示已知列；支持 AS、JOIN、前置 SELECT 列表达式、部分列名前缀及内层同名别名遮蔽。CTE、派生表、歧义别名和不完整到无法解析的语句不猜测原表。Redis 不提供关系列补全。候选包含列名、类型和注释，替换仅覆盖点号后的单词，保留别名；列名按方言安全引用。字段候选最多 1024 项，不使用普通关键词的 60 项限制，实际元数据仍受驱动上限约束。

字段首先复用同连接/库的表结构缓存及 Note 查询取得的完整原表结构；缓存最多 32 个表，按连接代次隔离，断开、配置保存及目录刷新时清理。若缺少字段，点击单元工具栏“读取字段”明确通过已连接会话加载字段（不查询行、不新建连接），完成后刷新提示；Ctrl+Space 可主动显示已有提示。读取不改变库对象面板选择。尚未连接时需先连接，遵循不为补全隐式联网的规则。

共享数据网格将组件外层单元格 padding 设为零；文字使用单元格内边距，高亮和焦点框覆盖完整单元格。右键命中包装层填满行高，表头、行号与数据格使用一致的高亮范围；保留细网格线、虚拟滚动和主题/自定义颜色。该修复同时作用于 Note 结果和“打开表”网格，不截断复制或编辑的原始内容。


## 阅读态复制与 SQL 语法着色

阅读态及折叠单元右侧提供复制图标，复制 SQL/Markdown/任务的完整原始正文，保留注释、换行和空白，不包含连接配置、查询结果或单元注释标题，不进入编辑态。SQL 编辑器和阅读态代码块共用注册的 tree-sitter-sequel 语法：关键词、函数调用、字符串和注释按当前深浅主题着色；语法着色不等于数据库执行或语义验证。仅添加 SQL 语法模块，没有开启组件的整套语言依赖。

本轮 Windows 隔离 SQLite 窗口已验证：阅读/折叠态复制 SQL、Markdown、任务的原始正文；SQL 编辑与阅读着色随深浅主题切换；查询结构缓存命中 u. 列候选、Enter 插入列名，以及显式读取字段后自动刷新且 .note 内容不变。网格列、行号、数据格和表头高亮均铺满内部区域，保留 1px 网格边界。CTE/派生表字段推断和真实 MySQL/PG 字段读取未作本轮服务端验收。


## 草稿目录与打开文件夹

未指定自定义目录时，新 Note 写入应用数据根目录的 `saved`：Windows 安装模式为 `%APPDATA%\TurboDbNote\saved`，绿色模式为程序旁 `data/saved`。保留既有数据根标识，不因品牌大小写迁移路径。旧 `notes/drafts` 与索引已有目录继续发现，不自动搬移、删除文件；设置中的自定义路径优先。打开所在目录使用后台系统文件管理器调用，路径独立传参；目录不可用或启动失败时显示反馈。

本轮 Windows 隔离窗口验证旧 drafts 文件恢复、新建 Note 写入 saved，以及 Note 右键实际打开对应 saved 文件夹。Redis 库对象在隔离 Redis 5.0.14.1 上验证筛选、前缀展开、双击值页与 JSON／Hash 展示；适配器另验证 String、Hash、Set、ZSet、List、二进制和大值截断，不代表 Redis 6/7 ACL 或集群已验收。

SQL 阅读态复制按钮常驻代码框右上角，代码右侧预留 44px，不遮挡长 SQL；无注释标题时不生成独立标题行，有注释则保留。复制完整原始正文，按钮与双击编辑命中区分开；折叠态仍使用摘要行右侧复制。

已在 Windows 隔离窗口验证单行／多行及带注释标题的 SQL 布局、深浅主题、完整原文复制，并确认连续点击复制按钮不会穿透进入编辑态。


## 连接就绪、字段读取与删除

已指定有效连接／数据库的 SQL 单元在打开或切换范围时后台连接；沿用已保存配置、会话缓存／加密凭据，不读取或替换连接编辑草稿，不切换数据库侧栏选择。同一配置自动尝试一次，共用最多八个数据库连接；失败或手动断开后不循环重连，明确点击 Col 可以重试。恢复笔记可以连接，但不会执行笔记 SQL。

Col 使用蓝色列标识，F 使用主题辅助色，执行为绿色、停止为警示色。Col 明确读取当前表／别名的列；等待连接和当前操作结束，最多 60 秒，内容修订、范围或连接代次改变则放弃排队结果。补全本身仍只使用本地缓存，不因接受候选而联网或执行。字段回调仅刷新当前编辑提示，不改写文档。

左侧笔记右键增加删除，二次确认说明目标与恢复方式。删除前必须完成保存、停止任务及处理未提交结果；删除期间锁定文档，后台校验文件并移动至同目录 `.turbodbnote-trash/<UUID>-<原文件名>`，成功才移除索引和页签；失败保留界面与文件。外部修改冲突不删除，回收目录不被草稿扫描重新发现。此功能不是系统回收站；恢复时手动将文件移回并打开，不自动覆盖已有文件。

本轮 Windows 隔离 SQLite 窗口验证：打开 Note 自动连接、多个单元共用一个浏览连接，Col 读取 u. 别名列后展示候选且文件内容不变；手动断开后保持断开，明确点击 Col 可重新连接并读取字段。删除取消保留原文件，确认后关闭页签、移除列表项，回收文件逐字节一致，移回并刷新可重新发现。MySQL／PostgreSQL 自动连接及繁忙操作排队尚未做真实服务端验收。


## 执行计划与单元插入图标

编辑工具栏使用执行三角右上角 P 表示执行计划，加号右上角复制页表示复制插入；复制插入之后紧跟前插、后插，再是上移、下移和红色删除。复制插入为绿色、前插蓝色、后插紫色，沿用深浅主题的 Note 图标色板；提示、禁用条件、二次确认和插入新 ID 的行为不变。

直接执行 `EXPLAIN SELECT ...` 与计划按钮共用只读计划结果。已有 EXPLAIN 时不再次添加前缀，不附加 LIMIT／OFFSET、不按默认 20 行裁剪计划；上限仍为 1000 行／512 列。支持 SQLite EXPLAIN／EXPLAIN QUERY PLAN、MySQL EXPLAIN FORMAT=JSON、PostgreSQL 不执行查询的格式等选项；SQL 保持原文。ANALYZE（包括括号选项）、嵌套计划、写入语句、写入型 CTE、文件输出及锁查询仍拒绝。当前计划入口仅支持查询，不表示所有数据库版本／扩展选项均已验收。

本轮 Windows 隔离 SQLite 窗口验证直接 F5 执行 EXPLAIN QUERY PLAN，以及已有 EXPLAIN 再点击计划按钮；复制插入生成不同 ID，前插／后插分别出现在正确位置，深浅主题图标与顺序符合设计。MySQL／PostgreSQL 的原文传递、选项和风险拦截采用适配器模拟测试，未作真实服务端验收。


## 阅读模式与结果文档

模式切换保留左侧最前面，使用 Note＋笔图标，编辑态显示绿色眼睛按钮和选中底色；阅读态显示蓝色笔按钮，嵌入内容框左侧，不占额外工具行。保留 F2／双击编辑、Esc 阅读及完整复制。

结果工具栏最前提供表格／JSON 图标，共用 ResultDocument 只读组件。单值对象／数组形态的计划结果自动进入文档视图；无效 JSON 保留原始文本。普通表数据及 Note SELECT 默认表格，切换只改变展示，保留分页、排序、选择与暂存修改；当前页以 columns／values 展示，避免重复列名覆盖并保留 NULL，不猜测文本列的数字类型。单值展示其正文；表格右键“查看完整内容”展示选中值。Redis 值页复用此组件，但仍受适配器既有读取上限限制，不能把完整复制预览误认为读取了服务端未返回的数据。

后台格式化保留 JSON 数字原始字符，不经浮点 Value 重写。显示缓存上限 256 KiB、10000 行、每行 4096 字符，触限明确提示；搜索仅覆盖显示预览。虚拟行支持对象／数组折叠，搜索显示匹配行；复制使用当前结果快照的完整原文（或全部已加载页的 columns／values），不受显示上限限制、不请求全库。文档不写入 .note，也不执行或编辑数据库数据。

本轮 Windows 隔离 SQLite 窗口验证：阅读态图标进入编辑、切回阅读后查询结果保留；Note 查询与数据库表格均能切换文档，搜索到第 100 个数组项、折叠／展开、拖动滚动条到末尾，完整复制与原 JSON 逐字符一致。新增暂存行在 JSON／表格之间切换后仍存在，撤销前未提交数据库；右键完整内容入口可用，文档内容未写入 .note。MySQL EXPLAIN FORMAT=JSON 自动展示按单值结果识别，本轮未连接真实 MySQL／Redis 验收。


## 图标与 AI 草稿交接

模式切换图标表示目标动作：编辑态显示绿色独立眼睛（轮廓眼形＋实心瞳孔）（切换阅读），阅读态显示蓝色 Note＋笔（切换编辑）。Note 主工具图标统一 16px、点击区域 24px，新增类型菜单使用同尺寸图标；新增／读取／保存为蓝色，执行为绿色，格式化／AI 为紫色，停止／删除为红色，布局操作保持中性色。

JSON 滚动条轨道与内容使用同一视口尺寸，避免缩短 14px 产生伪溢出。内容宽度按当前等宽字体的实际排版测量，字体、字号、搜索或折叠改变时更新缓存；横纵轴独立按溢出显示。

单元格 AI 图标位于执行计划之后、复制插入之前。点击展开右侧聊天，将原始 SQL 放入安全长度的代码围栏，并附带单元实际生效的连接名称、数据库与驱动类型；界面上下文同步对应引用，不含密码、连接串或结果数据，不连接 AI、不发送消息、不执行 SQL。草稿上限沿用 64 KiB，超限提示且不截断。

已有草稿可追加／替换／取消；已有明确范围与新范围不同则禁止追加，避免混用。确认期间若草稿或智能体变化则保留原内容并提示重试。Codex／Claude 的草稿及范围独立保留，范围只是供 AI 分析的上下文，不赋予数据库执行权限。

本轮 Windows 隔离窗口验证：彩色图标及新增类型菜单尺寸一致；JSON 未溢出不显示水平条，折叠后两轴无伪滚动条。SQL 原文、围栏和连接／数据库信息已核对 AI 草稿；追加与替换内容通过完整复制校验，取消保留旧草稿，不同范围禁用追加。聊天仍停留未连接／未发送状态，没有调用真实 AI 服务。修复长草稿与输入框底部工具栏重叠，并保留清空和右键菜单。


## SQL 行注释

SQL 单元允许注释行与一条可执行语句混合。MySQL 支持 `-- 注释` 和 `# 注释`，也支持前导空格及行尾注释；SQLite／PostgreSQL 使用 `-- 注释`。MySQL 的 `--` 后应保留空格，不把其它方言的 `#` 运算符改写为注释。复制和存储保留注释原文，字符串中的 `#`、`--` 仍为数据。注释中的分号不算多语句，分页前使用换行避免追加的 LIMIT 被行尾注释吞掉。MySQL 范围使用注释感知的着色入口，`#` 与 `--` 共用主题注释颜色；引号字符串、反引号标识符和块注释内的 `#` 不会作为行注释起点。编辑和阅读态共用此规则，其它方言仍使用原 SQL 着色器。


## MySQL 表结构查询

`SHOW CREATE TABLE/VIEW`、`SHOW INDEX/INDEXES/KEYS FROM/IN table`、`SHOW [FULL] COLUMNS/FIELDS FROM/IN table` 进入独立只读结果通路，支持限定库名及结尾注释。原 SQL 直接交给适配器，不追加 LIMIT、不进行 COUNT、不开放数据编辑；结果上限 1000 行／512 列，不按 Note 20 行页大小截断。此窄范围支持不包含 SHOW 任意选项／表达式过滤，也不放开会话控制、执行型注释或多语句。截图中的 SHOW CREATE TABLE 报错源于之前未列入白名单，不是 # 或 -- 注释引起。

本轮验证：11 项 Note 查询回归与两项 SQL 着色测试通过，截图原句 SHOW CREATE TABLE 加两种注释的原文传递由适配器模拟验证；Windows 原生窗口检查 MySQL 范围编辑／阅读态的 # 与 -- 着色以及深浅主题，SQL 文档内容未改变。未连接用户服务器，真实 MySQL 权限／版本仍需目标环境验证。


## 应用策略、统一索引与目录

默认语句包含既有查询、EXPLAIN、确认后的表级写入，以及 SHOW CREATE TABLE/VIEW 和 SHOW INDEX/INDEXES/KEYS/COLUMNS/FIELDS。SHOW TABLES、SHOW STATUS、SHOW VARIABLES 为可在设置 → 数据库 → SQL 勾选的额外只读类型，不接受任意 SQL 前缀。默认关闭扩展时明确提供设置入口；服务器权限失败仍需服务端授权。阅读态与折叠摘要的 SQL 单元提供 AI 入口，复用原有草稿交接。

应用运行元数据统一为 connections.db：连接/密码保持原表，settings、navigation、note-library 使用 app_documents 的独立版本行。迁移优先读取旧 settings.db，缺失时读取 settings.json；导航与笔记索引各自从旧 JSON 一次导入。旧文件保留但不再双写。正文、聊天历史与日志仍为独立文件。连接密码继续保持原 DPAPI 引用，未知时间不补造。

目录选用 saved：比 notes/drafts 更短，也符合未命名 Note 已自动持久化的实际行为；notes/drafts 的优势是分类更直观，但 draft 容易被误认为可清理临时文件。保留 saved 默认、自定义路径优先、旧 drafts 继续发现，不自动搬移或合并同名笔记。

本轮 Windows 原生窗口与隔离 MySQL 8.0.32 验证：自动握手连接、默认 SHOW CREATE TABLE、SHOW TABLES 禁用提示及直达 SQL 设置、勾选后执行和重启后保持生效；阅读态 AI 交接正确。settings/navigation/note-library 三个命名空间落入 connections.db，旧 settings.db／JSON 保留且未双写，新建未命名 Note 继续进入 saved。MySQL 4.x／5.5 仅新增握手路由与不发送凭据的协议模拟验证，本轮没有这些旧版真实服务端验收；TLS／旧认证仍保留明确确认。

## 编辑辅助、表定义与 AI SQL

SQL 编辑态检查当前连接，并为别名列补全按需加载元数据；候选显示类型标识、列名、类型与注释。自动尝试有界，连接失败后可通过 Col 明确重试。

表定义面板提供独立筛选框和表选择，MySQL 使用 SHOW CREATE TABLE/VIEW 单独读取 DDL，不要求读取索引或触发器的附加权限。Note 工具条 DDL 与表右键菜单打开同一面板，文本可选择复制。

SQL 命令设置默认开启 SELECT、EXPLAIN 和受支持的 SHOW 只读命令，写入命令需显式开启；确认后执行前再次检查命令许可。AI MCP 使用相同许可，逐次确认执行及分享有界结果，不提供账号、密码、主机或连接串。当前协议采用 MCP Streamable HTTP 2025-06-18；连接与停止分别创建／撤销临时本机端点。真实 AI 服务端调用需单独验收，协议测试不能替代该验收。


AI 排版按 Markdown AST 拆分编号／无序列表。列表项开头的加粗标题独占一行，正文与代码另行呈现；只影响阅读布局，“复制全部”保留原文。未支持的行内 HTML 片段转义展示，避免渲染器吞掉文本。本次通过 Codex 0.156.1 → 本机 MCP → 隔离内存 SQLite 的真实 EXPLAIN 测试；未使用用户业务库，不等同于 MySQL／PostgreSQL／Claude 完整验收。

## 任务清单

任务单元支持最多 1000 个独立任务项：名称、完成状态、负责人文本、可选截止日期与本地时间、到时或提前 5／15／30／60 分钟提醒。点击名称、负责人或日期编辑；完成项置灰并加删除线，取消勾选恢复。任务项删除须确认。提醒仅针对应用运行期间已打开的笔记，约 10 秒检查一次；完成项不提醒，错过的提醒补发一次，投递时间记录随 Note 自动保存以去重，应用内提醒保留到用户关闭。应用关闭或笔记未打开时不保证投递，不发送外部消息。

负责人是本地文本，不代表账号绑定或消息分配。旧正文保留，逐行转换必须显式点击；原说明仍可查看、修改和复制。新保存文件使用 Note 格式 v2，兼容读取 v1，旧应用会拒绝 v2 而不能静默丢失清单字段。项目、连接引用及 SQL 恢复不变，恢复提醒不执行 SQL。复制单元会生成独立任务项 ID。

## Markdown 编辑工具栏

MD 单元编辑态显示标题、加粗、斜体、删除线、行内代码、列表、链接、引用、代码块、表格、分隔线和图片链接，所有格式按钮统一图标＋悬停提示并直接显示，不再提供“更多”菜单；标题与列表保留下拉选择。窄窗口下工具栏自动换行。多行选区支持列表及标题替换，中文和 emoji 按编辑器 UTF-16 选区安全映射；操作触发原生撤销／重做和既有自动保存。连续点击同一文字／列表格式可取消最近一次格式操作。链接插入后选中 URL 占位，工具栏及菜单操作后焦点回到正文。Ctrl+B／Ctrl+I／Ctrl+K 限定在 MD 正文输入框。图片只生成链接语法，不自动上传；插入代码块不执行代码或 SQL。

## MySQL 数据库检测与人工诊断

连接检测发送 SHOW DATABASES，元数据请求期间使用 UTF-8 返回字符集。列带 BINARY 标记时复用对象浏览的严格 UTF-8 读取，跳过列类型匹配但不采用有损替换；列表失败显示警告而非“检测到 0 个数据库”的成功提示。服务端错误继续按安全错误码分类，不记录原始错误或凭据。

Note 新增 SHOW DATABASES/SCHEMAS，并支持 SHOW TABLES/STATUS/VARIABLES 的 LIKE 单引号字符串筛选；STATUS/VARIABLES 支持 SESSION/GLOBAL。均走有界只读元数据通路，不追加 LIMIT、不开放结果编辑。命令许可复用设置 → 数据库 → SQL：已有显式许可列表缺少新命令时仍需人工勾选。LIKE 字面量不接受反斜杠转义，避免 SQL 模式差异改变语句边界；单引号可用双写转义。WHERE 表达式、会话控制、多语句、执行型注释仍不允许，AI 模式权限不扩大。

本次 BINARY 标记、中文库名和非法 UTF-8 通过本地 MySQL 协议模拟验证；未连接用户 MySQL 8.4.5 服务端。用户提供的人工查询已证明其账号可列出数据库，不需要据此扩大账号授权。

本轮最终 Windows 隔离窗口已验证：默认范围回填不取消旧索引升级、实际风险预授权与取消、真实 MySQL 顺序执行、外部 DDL 发生后在写入前停止、重新授权后成功、USE 目标不符弹出选择框、禁用按钮与语句警告悬停提示。验证程序使用最终 target/debug 构建的隔离副本，诊断与 AI 自动连接均关闭；结构化输出日志未包含测试 SQL 正文。

脚本清单每页 6 条（27 条分 5 页），右侧复用常显策略的滚动条；一页内容刚好容纳时没有可滚动范围。点击警告计数可仅看不支持项，保留原编号、源码行号及具体原因，支持上一／下一问题和清除筛选后定位当前项。过滤只扫描磁盘固定长度索引、读取当前页摘要，内存不保存全量问题 SQL。预览区右上复制图标复制当前原始语句（含注释／版本标记），不复制结果、不执行 SQL。Windows 隔离窗口已验证 27 条分页、两条问题筛选与前后切换、恢复当前页、复制图标与剪贴板完整正文。

问题筛选、清除与前后定位整合到执行工具栏右侧，不单独占一行；正文复制保留在预览区右上角。MySQL 脚本增加 ALTER TABLE … ENABLE KEYS / DISABLE KEYS 的严格识别（包含版本条件注释），按 ALTER 许可、预授权及结构校验执行，效果由服务器和存储引擎决定。TDBSQL01/02 派生索引会后台升级为 TDBSQL03，不改写原 SQL，也不需要重新导入。独立 MySQL 8.0.32 已实测包含锁定、DISABLE KEYS、写入、ENABLE KEYS、解锁的脚本。

批量脚本执行前的许可检查汇总缺少的命令类别，每类列出首条原语句编号与源码行号。提示框明确此时尚未执行，提供关闭、定位首条语句、打开 SQL 权限设置；定位保留完整原文，设置跳转复用既有入口，不自动勾选、不自动重试。通用许可错误使用 SQL 命令文案，不再把写入称为元数据查询。运行过程中许可变化仍停止后续执行，不能把运行后停止解释为所有语句都未执行。

权限反馈的 Windows 隔离窗口验证：同一提示列出第 2 条 INSERT 与第 3 条 DROP，数据库内容未变；定位入口选中完整原语句，设置入口直接进入数据库 → SQL，权限保持未勾选。

## 整份脚本一次授权

冻结资源和目标、验证命令许可并采集结构指纹后，打开一个“确认执行 SQL 脚本”窗口。显示连接／库／环境、读取／数据写入／结构修改／会话设置／表锁／事务统计，突出删除／清空和关闭约束检查；原始语句每页六条，点击按需读取完整正文，可筛选“仅看需授权语句”。底部仅“关闭／授权并执行”，不再逐项弹窗，也不提供永久授权。取消／关闭不执行正文，恢复文件不重放。授权绑定脚本哈希、执行范围、连接快照；每条执行仍重查许可和结构，失效停止，不自动再次授权或重试。普通 SQL 单元的原有预览后二次确认保持不变。

单次批量授权已通过 Windows 原生隔离窗口验证：SQLite 关闭弹窗不建表，一次确认执行建表／插入／更新／查询；独立 MySQL 8.0.32 一次确认执行包含变量保存恢复、关闭外键检查、表锁、ENABLE／DISABLE KEYS 和中文写入的十条导出 SQL，并验证分页与仅看风险。其它版本及 Linux GUI 未据此验收。

最终批量授权窗口仍验证结构变化保护：打开确认框后由独立连接修改目标表，单次授权后在首条写入前停止，后续 INSERT 未执行。串行全工作区测试通过；首轮既有 Windows 凭据迁移测试偶发失败，单独与串行复测均通过。

脚本标题的文件图标支持复制 SQL 文件路径，指向 Note 同目录 `.turbodbnote-scripts` 内实际使用的托管副本，不伪造原始导入路径。执行失败常驻显示原语句编号、源码起始行、失败阶段和已完成数量；定位关闭问题筛选、跳转相应六条分页，并展示本次冻结执行的完整 SQL，可沿用预览复制。位置是语句起始行，不声称为服务端指出的具体错误字符；执行准备及写入后校验分别标注，写入结果不确定时仍不得自动重试。SQL 正文和原始服务端错误不写入输出日志。

本次 Windows 隔离 SQLite 原生窗口验证：路径剪贴板与托管文件完全一致；第七条重复主键写入失败显示起始行八、已完成六条，定位跳转到第二页并选中第七条，复制得到完整冻结正文，最近结果仍明确标注第六条。此次不代表 MySQL／PostgreSQL 或 Linux GUI 的服务端验收。

## 脚本执行报告

单元底部显示成功、失败、未执行语句，以及新增表／跳过建表、INSERT／UPDATE／DELETE 影响行。原生 MySQL／PostgreSQL／SQLite 使用驱动结果中的影响行数；无法提供时仅 MySQL／SQLite 尝试已有会话计数器，失败显示未知而不是零。INSERT ON CONFLICT／ON DUPLICATE KEY UPDATE／REPLACE 等混合写入单独统计，不把影响行数解释为纯新增数量。UPDATE 采用驱动影响行口径，不推断实际改变的记录数；事务回滚不改写历史成功语句计数，概要明确不等于最终提交数量。

普通 CREATE TABLE 成功可计为新增；MySQL 条件建表在其它语句清除诊断前读取 SHOW WARNINGS，代码 1050 表示跳过，诊断缺失／截断不能确认时显示未知。SQLite／PostgreSQL 条件建表成功本身不能证明新建还是跳过，保留未知，不根据存在性快照猜测并发结果。

“执行明细／查询结果／表结构”共用现有只读虚拟网格，明细仅保留最近 500 条，统计累加所有成功结果，表结构最多登记 128 个目标；不为每条语句创建新 Note 或网格实体。DDL／会话结果不替换最近真实行集，不再显示空查询网格。运行中网格更新节流，停止后刷新；错误定位沿用原有语句／源码起始行入口。报告和查询数据不保存到 .note，重跑清空上次运行报告。

表结构仅在脚本停止后由用户选择目标读取，复用适配器的列元数据接口，显示字段、类型、主键及注释，最多 512 个字段并提示截断，不查询表数据或自动执行导出 SQL。后台任务绑定运行修订与连接快照；切换目标、重跑、关闭取消任务，过期结果不覆盖界面。结构读取失败不改写已完成脚本的状态，输出仅记录元数据任务类别、计时及结果，不记录 SQL 或服务端原始报错。

Windows 原生隔离窗口验证：SQLite 插入／更新／删除影响行 3／2／1，ALTER 后仍保留前一次两行查询结果并可查看三个字段；独立 MySQL 8.0.32 验证新增表 1／跳过建表 1、相同 DML 计数、中文字段注释及主键。最终窗口验证明细滚动条常显并可滚动至第七条；首条写入失败显示失败一条／未完成一条，不沿用其它单元查询结果。此验证不代表 PostgreSQL 真实服务端或 Linux GUI 验收。

最终可执行文件补充验证：失败明细保留 INSERT 命令、目标表和请求耗时，失败后仍可按需读取已识别目标的字段；零行查询明确显示本次语句编号及零行，不保留上次非空结果。

MySQL imported scripts support `DROP TRIGGER [IF EXISTS] [database.]name` through the script-only `script_drop_trigger` permission (off by default). It requires batch write authorization and rechecks bounded trigger metadata before execution. Cross-database targets, multiple statements and other trigger DDL remain rejected. Ordinary SQL cells and AI tools do not gain this capability. Deleting a trigger changes subsequent writes; DDL cannot normally be rolled back. Script index format 4 reparses older derived indexes without changing the source.

MySQL script context also accepts transaction isolation levels, integer user-variable assignment and variable +/- integer, plus SELECT INTO a single user variable before FROM. SELECT INTO uses the off-by-default script_variables permission; a terminal FOR UPDATE uses script_lock. Queries are inspected without INTO/locking only for validation and table guards; the original frozen SQL is executed. Stored/UDF calls, file output, unknown SET expressions and cross-database targets remain rejected. Parent session lifetime, timeout, batch authorization and explicit transaction completion apply.

Script-specific statement execution preserves the adapter’s sanitized error category instead of replacing every failure with a session error. MySQL 1142/1143, 1227, 1360 and 1419 have actionable categories; no raw native error text is logged. Leading comments are ignored for the execution-detail command label. This does not grant server privileges or retry writes.


Note access modes are session-local: Read only by default, Read/write for INSERT/UPDATE, and DBA for implemented DDL and script controls. They reuse the AI mode policy without sharing state. Effective commands intersect local mode, global SQL settings and adapter/server restrictions; DBA never grants server privileges. Closing/reopening resets to Read only. Ordinary cells, imported scripts and editable result-grid commits all enforce the mode. Running work, pending previews and unsaved grid edits lock mode switching. Import/recovery never executes SQL.

Connection selectors in Note defaults, SQL cells, AI chat and transfer source/target share environment swatches, driver icons, names and secondary environment/type/address text; IDs remain selection identities. File targets show filenames, never credentials.

Script lists summarize the first effective SQL instead of ordinary leading comments. Preview shows the executable line and original source range, with leading comments collapsed separately and Copy SQL / Copy full source. Source is unchanged for authorization/execution; strings, MySQL `-- ` and `#`, PostgreSQL nested comments and executable `/*!...*/` are distinguished. Preview reads are bounded per statement and only six list rows are retained. In-memory source fingerprints and prepare/execute-stage checks bind script responses to frozen requests; hashes/SQL never enter journals.

After MySQL script failure, **Read-only diagnostics** explicitly opens a separate short-lived connection. It reads server version, current account/database, binlog/trust policy and a small direct-grant summary. Full grants (which can contain authentication material on older servers) are never returned to UI or logs. Missing variables/grant access are reported as unknown, not absence of privilege; role-derived grants are not fully evaluated. Cancel, target change and close release the diagnostic session. It neither replays scripts nor changes server settings or privileges. Binlog verification uses an isolated local server; it cannot identify a remote gateway's behavior.

Connection setup and database operation deadlines are separate: the existing `timeout_seconds` remains 1–60 seconds, while `execution_timeout_seconds` defaults to 300 seconds (1–3600). Old connection records inherit the execution default without changing their saved connection timeout. Each operation includes its preparation/revalidation; a script uses one deadline per requested statement operation, not a single deadline for the entire file. Existing connection edits take effect in new sessions. Stop/close still cancels; timeout closes the dedicated connection and never retries writes. Native workers enforce the same contract (component protocol 5).

For Note only, Read/write allows ordinary query commands plus INSERT and UPDATE; DELETE requires DBA, global permission and confirmation. This covers SQL cells, script preflight/execution and result-grid commits. Deleting an unsaved draft row remains a local undo. AI chat retains its separate existing mode behavior.

Elapsed values remain milliseconds in protocols/journals and use one display formatter: below one second in ms, below one minute in seconds, then minutes/seconds or hours/minutes/seconds.


Global timeout defaults live in Settings → Database → Query. New connection drafts inherit them; pre-existing profiles without `inherit_timeouts` remain explicit overrides. Enabling inheritance retains the custom numeric values so disabling it can restore them. The global editor never rewrites connection rows. The shared service policy resolves a fresh concrete profile once at session start; runtime copies clear the inheritance flag so delegation cannot re-resolve a running session. Transfer endpoints resolve once when the job endpoint is created. Defaults apply to new sessions only; existing requests and connection generations remain valid.

Note permission feedback identifies the Note, current mode and blocked command. It prioritizes disabled global commands, then connection write restrictions, then the local mode ceiling. A persistent notification offers only the relevant action: open SQL settings, choose a compatible writable connection for the originating cell, or switch the originating Note to the required mode. The mode-switch action rechecks revision, current mode and pending work; stale guidance is rejected. Switching does not execute or retry SQL. Unsupported statements are not presented as solvable by elevation. Known script server-permission failures link to existing read-only diagnostics. These guidance rules are shared project conventions in AGENTS.md.

The center editor tab toolbar reuses NoteLibrary::new_note for direct draft creation and normal routing/persistence. Definition metadata flags are nullable to preserve compatibility with older workers; missing flags display an em dash, not false.

GBK raw-read compatibility is per connection and defaults off. Row reads opt into a dedicated Access query path; metadata stays UTF-8. C-client field charset IDs distinguish latin1_bin text from binary charset 63. UTF-8 fields retain UTF-8 decoding, invalid/unsupported text fails closed. The helper handshake explicitly acknowledges this capability. Server data and schema are unchanged.

Raw GBK compatibility uses offset pagination: decoded text must not become a binary-storage keyset cursor. It does not rewrite SQL literals or repair stored encodings; predicates on misdeclared columns may require explicit HEX/binary matching.

Table query Notes use the same SQL cell and permissions as ordinary Notes but contain exactly one SQL cell; their table origin is optional document metadata. They preserve SQL edits, completion, auto-save and result-grid behavior. The filter builder loads only column metadata, uses the data-grid filter compiler and asks before replacing SQL. It does not execute generated SQL. Opening/restoring such a Note never runs a query. MySQL raw-GBK Unicode predicates require explicit SQL conversion rather than guessed character rewriting.

Table designer/data/comparison tabs restore only target metadata and comparison input drafts. They require an explicit Load tab action after the saved connection is ready; generations and write grants are recomputed from the current connection. No query result, credential or DDL draft is serialized into workspace layout.


## 表查询笔记复用

“查看数据（笔记）”按原始连接、数据库、schema 和表识别当前已打开的专用笔记；重复进入激活原页签，保留手改 SQL 和暂存结果。来源标记与可编辑默认范围分开保存，旧文件缺失标记时仍可按原有范围识别。仅本次明确点击入口时进入编辑并刷新安全 SELECT；写入、加锁、多语句和未审核函数调用不自动执行，暂存修改继续要求先处理。恢复页签只恢复文档，不自动查询。

专用单 SQL 笔记的编辑区高度有界，结果填充剩余空间，普通笔记的结果高度保持原行为。筛选生成 SQL 与数据筛选共用条件结构；按列类型输出数字，普通文本用引号，LIKE 字面通配符保留转义，反斜杠和控制字符保留安全编码。复制／替换 SQL 仍经预览确认，不隐式执行。

SQL 阅读态右侧提供执行/停止、执行计划、AI 与复制图标，复用编辑态的权限和确认流程。清除结果合并至结果工具栏，保留 SQL；运行中或存在未提交修改时不清除。

## 单元折叠与长 SQL 滚动

阅读态提供当前单元的展开／折叠图标，复用全局图示；折叠保留首行摘要、查询结果、草稿及会话，展开恢复内容。阅读内容使用 Note 外层虚拟列表滚动，外层滚动条在溢出时常显。

SQL 编辑器按实际 Note 内容区高度的 55% 自适应，上限 520px，至少保留三行编辑空间；超出时复用输入组件内部纵向滚动，关闭自动换行时保留横向滚动。鼠标位于编辑器内优先滚动 SQL，内层到边界才交给外层；工具栏不随 SQL 正文滚动。编辑器实体跨阅读、折叠及切页保留，窗口尺寸变化不重建草稿或会话。

清空 SQL 使用顶部工具栏阅读切换后的橙黄色橡皮擦图标，内容为空时隐藏；移除编辑器右侧重复入口。清空前确认，绑定当前修订并复核可编辑状态，通过原编辑历史清空全文，支持撤销；保留单元、连接与结果，已有结果按正常编辑流程标记过期。取消不改内容，运行或批量锁定期间禁止清空。

滚轮协调在捕获阶段记录外层位置，尊重内部控件消费事件；未消费时只提交本次滚动增量并刷新 Note。外层滚走时释放光标焦点而保留编辑实体，避免离屏编辑器牵引页面。虚拟列表仅补测失效高度，复用有效缓存；两条滚动条使用独立槽位。Windows 隔离 SQLite 窗口验证包含 100 行 SQL、31 个单元、内层／边界／外层往返、折叠展开、缩放及清空取消／确认／撤销；验证脚本逐次同步投递滚轮，避免异步事件积压造成提前截图。


## SQL 单行工具栏与结果滚动修复

SQL 单元的阅读切换、清空、注释、连接和数据库选择与常用操作保持同一行；字段宽度有界，复制／插入／移动／删除收进“更多”，仍复用原单元事件、暂存保护与删除确认。窄视口工具栏仅横向滚动，不折行；垂直滚轮继续交给 Note，横向位移由独立滚动条处理，字段原始值不截断。

Note 内容区固定约束宽高并保留独立外层滚动条槽位。内部表格／文档确实发生有效位移时保留外层锚点，到边界时继续滚动 Note；将表格原始越界偏移与实际可见位移区分。内容缩小到不溢出时修正原越界锚点，避免首段被裁掉而滚动条已隐藏。

结果视图仅在忙碌或暂存状态切换时通知单元；可见行依靠正常布局重测，选择、悬停和网格滚动不再反复将其高度置零。切换已有单元的编辑态保留列表测量；新增单元才重建结构并定位新单元，草稿、查询结果和会话实体继续保留。


本次 Windows 隔离窗口回归覆盖两个 SQL 单元与分页结果、结果区到边界后的内外滚轮、外层滑块上下拖动、切换第二单元编辑、约千像素窗口单行布局、开启侧栏后的工具栏横向滚动、折叠后内容完全可见及展开恢复。更多菜单验证取消删除、复制新 ID 且不执行 SQL；T／Col 弹层在受约束工具栏内仍可正常插入。查询使用隔离 SQLite；网络数据库选择器仅用本机占位连接验证字段布局，不代表真实网络服务端或 Linux GUI 验收。


## 文件级滚动与编辑器高度

外层列表的滚动范围覆盖整份 Note，不将正在编辑的单元注册为列表焦点锚点。拖动文件滚动条将导航焦点交回 Note，但保留单元实体、草稿和撤销历史。SQL 编辑器关闭代码末尾额外半屏留白，以实际视觉行（包含软换行）与统一行高测量高度，较短内容自然展开；长内容以文件视口的可用高度限高，内部到边界后继续滚动整个文件。组件使用仓库内固定 0.5.1 适配，不修改全局 Cargo registry，见 vendor/gpui-component/TURBODBNOTE-PATCH.md。

补充 Windows 隔离窗口验证：较短 SQL 完整显示且没有末尾额外滚动区域；长 SQL 内部滚动与文件滚动衔接；第二单元保持编辑状态时，外层滑块可拖回首单元。查询仅使用专用 SQLite，草稿内容和查询目标未改变。

## 常驻新增与文件导航

普通 Note 的 SQL／MD／任务新增入口固定在内容区底部，始终可见，不再作为虚拟列表的末尾占位项；新增仍追加至文件末尾，遵循批量锁定和单元数量限制。文件滚动条旁的向上按钮返回整份文件开头，保留编辑草稿和单元实体。独立表查询页维持单 SQL 单元模式，不显示新增入口。

文件顶部连接与数据库字段有固定宽度，数据库输入框宽 180px，清空和下拉保留；选择菜单宽度独立于字段。工具栏保持单行，内容区过窄时使用独立横向滚动条，避免数据库字段挤占整行或换行；不截断实际数据库值、不修改默认目标。

Windows 隔离窗口验证覆盖底部按钮分别追加 Markdown／SQL 新 ID、回顶返回原首段、原单元内容与目标不变、数据库下拉弹层及缩放后的单行工具栏。文件范围使用专用 Note／SQLite，网络数据库字段使用本机占位目标验证布局，不代表真实服务端查询或 Linux GUI 验收。

Note 顶部默认范围与每个 SQL 单元的数据库候选列表统一应用连接所保存的数据库过滤规则，额外发现的名单和原选中值也不作为规则绕过入口。被规则隐藏的已保存数据库目标仍保留在输入框，不自动切换、不清空、不改写文档，也不把候选过滤变成 SQL 执行权限。规则修改后候选列表随连接状态通知刷新。

## 普通 SQL 的执行范围与多语句转换

普通 SQL 单元在无选区时执行光标所在语句，有选区时要求完整语句；字符串、注释及 PostgreSQL dollar quote 内分号不拆分。分段复用脚本词法扫描器，执行后分页冻结本次 SQL，不随光标改变。单元超过普通 SQL 字节上限或包含无法安全分段的过程体，提示使用脚本模式。解释计划遵守同一执行范围，接受补全仍优先。

SQL查询选中多条或无选区执行时，在原单元创建运行期冻结快照，保留正文、类型和单元 ID。风险批次预览冻结目标、语句后单次授权；纯只读批次直接执行。转换和拆分入口已移除，旧文档标签兼容保留。

Windows 隔离 SQLite 窗口回归覆盖：同单元三行文本中的光标语句独立执行、完整多语句选区引导、拆分后新 ID／原文拼接与范围保留、脚本附件原文校验、转换无自动查询，以及明确脚本授权后两条 SELECT 的顺序成功报告。临时 SQL 写句柄在准备脚本前关闭，补充 Windows 附件发布回归，避免共享模式冲突。此验证不代表真实 MySQL 多语句或 Linux GUI 验收。

## 输入优先的高亮与缓存补全

普通 SQL 输入即时更新文本、光标及最小布局，不在逐键链路执行 Tree-sitter 解析或捕获查询。空格、换行及 SQL 标点后按下一帧调度后台高亮，其他输入停止 150ms 后补齐；输入法标记阶段暂停。每个编辑器只有一个合并工作任务，缓存仅在文本修订、语言及主题匹配时使用，旧范围不应用到新文本。长物理行在后台选择轻量语言；正文、复制、执行和撤销均保留完整文本。自动换行复用未变前段，只重排编辑前一软行及尾段。

自动补全采用 50ms 合并门控，关键词／函数使用驱动本地词库，词法上下文和别名解析在同一标识符内复用，范围外 SQL 或方言改变即重算。表名按每个完整前缀从当前连接／数据库的内存目录与已发布 SQLite 快照取匹配，不受首轮前几项限制；缓存至多 32 个范围／前缀组合。SQLite 读取只读、锁等待 20ms，不初始化或远程建目录，前缀索引在显式目录同步时创建。连接代次、在线状态、目录修订及规则变化使旧候选失效。

T 仅保留图标，无额外下拉箭头或独立表选择器，点击恢复原光标并显示最多 8 个缓存表名。继续同词输入保持表名模式，跨词、连接变化或正常手动补全退出；候选仅替换当前标识符，不执行 SQL，无缓存仅提示手动刷新。Col 保留独立的显式浏览／加载入口；自动列提示只读已确认原表的列缓存，不自动查询远程元数据，不推断派生表／CTE。浏览器或 Col 显式加载的列可供后续输入复用。

Windows 隔离 SQLite 原生窗口验证覆盖：T 在光标处显示最多 8 项、从 6200 多条已发布缓存目录中继续按完整前缀定位表名、带方言引号插入、无新增远程元数据请求／无连接其他服务器、自动关键词补全和 Tab 接受、长行与多单元输入后的完整自动保存、37KB 长行全量替换与中文输入后的单步撤销恢复。输入消息处理耗时不等于输入到屏幕呈现延迟；多单元布局仍有额外开销，未据此承诺所有文档均达到固定帧率。输入法组合阶段按状态暂停高亮，但尚未进行真实输入法及 Linux 原生窗口验收。

同机 Windows 开发构建的隔离样本（每场景 133 个字符）中，短 SQL 的同步 WM_CHAR 处理 P95 为 22.2→16.7ms，约 24KB SQL 为 51.0→27.5ms；两者自动保存完整。该数值仅反映消息处理往返，不包含真实输入到呈现的完整链路，也不代表发布构建、所有文档或真实数据库的性能保证。

## 同单元多结果与方向键布局

SQL查询的统一执行入口不修改正文、类型和单元 ID，正文上限 1 MiB。后台冻结执行文本并绑定修订、路径与连接快照，复用命令白名单、Note 模式、风险确认与顺序执行；失败停止后续语句，停止不撤销已提交写入。有选区执行完整选区，无选区执行本单元全部 SQL。结果缓存仍有界，超过 50 个行集或字节限制明确提示淘汰较早行集；执行语句数量不再与结果缓存数量混用。

每条读取结果使用独立只读 TableView，冻结 SQL 与连接，分别保留分页、选择和滚动位置，执行摘要保留写入影响行数与错误。分页会重新查询该冻结语句；执行过会话／写入操作或无法确认可独立读取的结果仅保留返回页，不在新会话猜测恢复变量、临时对象或事务。结果不写入 Note；缓存最多 50 项及约 16 MiB 文本，超出释放较早结果，单个响应另受适配器上限保护。

光标移动复用单份文本、可见物理范围、字体／字号、换行宽度、样式与背景均一致的塑形布局；改变任一项即重建。只裁剪绘制不可见软行，不截断正文、选区或复制内容。回顶入口与 SQL／MD／任务同排，移除内容右下角重复入口；返回文件顶部，保留单元草稿、光标位置和选区。

别名点号优先显示当前原表的缓存列。缺缓存时显示可选择的加载项，接受该项不插入文案；明确加载后检查修订、光标与连接代次，再恢复列候选。失败可通过 Col 重试，不循环重连、不推断 CTE／派生表，不因输入查询远程元数据。

输入组件绘制完成后仅在布局／滚动实际变化时补充通知，避免未变输入框不断安排下一帧；文本、选区、焦点、高亮发布和光标闪烁仍由各自状态变更触发。软行绘制按未滚动的实际视口裁剪，保留相邻行的字形边界。

Windows 隔离 SQLite 原生窗口验证覆盖：原单元两条 SELECT 经单次冻结授权执行，两个结果 Tab 独立显示；首个结果翻到第二页，切换后仍显示第二页，清除首个结果不清除第二个；正文与单元 ID 不变。点号缺缓存时不请求列，明确接受加载动作后显示并插入带方言引号的列名；未连接其他测试目标。底栏回顶恢复文件首个单元，37KB 长行替换、中文输入、单步撤销和完整自动保存均通过。

编辑态方向键的隔离开发样本（160 次左右键，同步发送按下和松开）中，短 SQL 往返 P95 为 75.1→59.6ms，约 24KB 单行 SQL 为 93.3→72.8ms；源文保持不变。这是消息处理组合耗时，不是实际键盘到首帧延迟，不能视为发布构建或真实前台使用的性能保证；极长单行仍有开销。先前在阅读态误采的方向键数字不作为编辑器结果。

以上不代表真实 MySQL/PostgreSQL 批次、Linux 窗口或真实输入法组合阶段已验收。

## 纯查询脚本无需授权弹窗

仅包含普通只读查询的脚本直接执行并展示多个结果集；混合写入、DDL、会话设置、锁、事务或其它风险操作仍需一次批量授权。两条路径均先检查命令许可、Note 模式和目标，执行时继续校验冻结正文、连接代次与表结构。

## 同文档视口与打开笔记列表

Note 向左右拆分时保留原栏，新增的视口共享原文档、自动保存、会话模式和运行期结果，不复制文件或重复执行查询。文件滚动位置分别保留，可对比不同单元；同一个单元的编辑与结果状态仍属于同一份文档。关闭任一视口而仍有其他视口时，不停止查询或提示关闭文档；合并分栏按原文档去重，最后一个视图关闭继续遵守保存和结果修改保护。布局恢复不执行 SQL，也不因阅读态恢复而连接服务器。

顶部更多菜单直接展示已打开笔记，可本地搜索标题、Note 所属项目、连接名与完整地址。每项一行显示标题、项目、左／右／左右位置；同一文件只一项，优先切换当前分栏中已有视图，否则聚焦另一栏，并滚动页签到可见位置。长内容截断并悬停查看完整信息。

显示标题与文件名分离：表右键创建显示“cq_user · 140.38:3306”，自动命名 Note 可附带默认连接短地址；用户手动名称优先。新文件采用 note-YYMMDD-N.note，后台按目录当天最大序号递增并使用不覆盖的原子保存；已有文件不自动重命名。

## 就地加载辅助目录

Col 不再打开库内表筛选器，而是聚焦原输入光标并请求原生补全菜单；u. 或 u.na 根据当前语句的别名解析列，未写别名时只提供该语句实际引用的表。候选保持列名、类型与注释，插入仅列名。缺列缓存显示“加载 表名 列名”，缺表目录时手动 T 提供“加载当前库表目录”。仅明确选择加载项后读取当前目标；未连接时可在列表内“连接并加载”，目录复用 SQLite 快照刷新通路，不跳转侧栏、不连接无关服务器。后台完成核对目标、连接代次、文本修订、光标和焦点，原位更新候选；移动光标、取消或切换范围后只保留缓存，不强行弹出提示。

本轮 Windows 隔离 SQLite 窗口验证覆盖：左右显示同一文件、通过独立文件视口对比不同单元的查询结果、关闭副视图与返回单栏保留原文及结果；按 Note 项目过滤打开笔记并回车切换；日期序号文件创建；Col 在别名点号处就地加载／插入列名，T 在缺目录及空前缀时显式加载后刷新候选，未连接无关目标。原有多结果页签、独立结果分页与详情展开回归通过。这里的独立结果分页指同单元内不同结果集；同一单元在双视图中的结果页签、分页与表格内部滚动仍共享，文件滚动分别保留。

全工作区单测串行复验 378 项通过、1 项忽略；既有 Windows 凭据清理测试在先前并行运行中偶发失败，单独与串行全量复验通过。本轮不据此宣称真实 MySQL/PostgreSQL 元数据服务、Linux 桌面或真实输入法组合阶段已验收。

Debug 与 Release 均已重编译；Release 原生窗口已复验项目筛选、回车切换、新文件日期命名、Col 就地加载以及返回单栏保留文档和结果。发行打包与发布不属于本次验证。

## 补全加载与接受按键

点击显式列／目录加载项后，列表保留并显示加载状态，焦点回到原编辑器；成功后原位更新候选，不再重复输入点号或点击 Col。来源单元不会被元数据通知与请求完成同时刷新。失败可重试；光标移动、Esc、点击外部、正文或目标变化使旧请求失效，仅保留安全目标缓存，不抢回输入焦点。

T 列表 Enter 固定插入选中表名。Tab 在新的空语句或独立表名前缀位置默认生成 SELECT，在 FROM／JOIN／UPDATE／INTO 或无法确认的位置仅补表名。换行不作为 SQL 分隔，字符串／注释中的分号不作为新语句边界，未闭合括号禁止整句生成；已有结尾分号和注释保留。列表底部的表名／SELECT 只切换本次 Tab 行为，保留选中候选，不增加工具栏下拉。两种方式仅插入正文，不执行；列提示和加载项继续支持 Tab／Enter，输入法组词不被补全接受逻辑截获。

本轮补全改动已通过 379 项全量测试（1 项忽略）、请求恢复校验回归和 Clippy。Windows 隔离 SQLite 窗口覆盖：鼠标加载列后无需重复触发即可选列，Tab 生成 SELECT，Enter 仅插表名，关系位置与未结束语句保护，切换本次 Tab 方式，多语句 Unicode 范围及一次撤销；接受补全未产生查询执行记录。测试中既有 MySQL 协议模拟曾遇到本机 TCP 10053 中止，单独与无窗口并发的串行全量复验通过；不代表真实 MySQL/PostgreSQL 或输入法组词阶段已验收。

本轮 Debug／Release 均已重编译；Release 已复验鼠标列加载原位刷新、Tab 生成 SELECT 与 Enter 插入表名。元数据恢复另有回归检查，允许刷新后重新读取新目录版本，但不放松正文、光标、连接代次或目标校验。未发布。

## 单元上下文工具与批量任务

SQL文件标题栏复用 SQL查询的连接／数据库选择器，默认继承 Note，可选择单元独立范围或恢复继承。数据库候选遵守连接过滤规则；更换目标不执行、不重写受管 SQL 文件，旧结果保留为过期快照，分页／提交禁用。运行及待授权锁定目标，USE 与选择库冲突继续停止。文件名截断、低频文件操作收入更多。

工具栏移除固定 880px 最小宽度，按实际栏宽切换紧凑布局；窄栏保留连接、数据库、T、Col、执行和更多，其余辅助动作及注释编辑按需打开。仍有溢出时保留可访问滚动。打开笔记菜单删除放大。

列候选使用图标、列名及灰色类型／注释。自动点号与 Col 共用解析，覆盖 FROM 前的 SELECT、JOIN ON、WHERE、CASE、函数、GROUP BY／HAVING／ORDER BY 与 DML 表达式；不同语句和 UNION 分支隔离别名，内层同名别名优先。CTE／派生表显式输出列在本地提供，简单单来源 SELECT * 可以透传；无法确认的表达式、表函数或递归输出不猜测为实际表。辅助解析维持 60,000 字节上限，在后台运行。

提示列表缺缓存时可连接并加载当前单元目标，成功后原位替换，失败可重试，取消／改文／移光标／切目标后不重开旧列表。可用本地目录允许离线辅助；数据库侧栏的离线目标搜索排除规则保持不变。接受列候选时核对实际列快照，其他目录刷新不得误把 Tab 变为缩进。

DDL 优先关联选中的真实表名（支持引用符及限定名称）或完整 SQL；无选区时使用当前语句、光标表名或别名列。单表直接展示，多表提供带来源行号的去重候选；未识别时明确提示，并提供“本单元中的表”和显式“从当前数据库选择”，不自动退回全库。无关未完成语句不阻断选中 SQL 的识别。点击时冻结正文、选区及连接范围，过期请求不得覆盖新目标；仅读取元数据，不执行编辑器正文。候选最多 32 张表，单元后备扫描最多 128 个语句范围；单语句深度分析限制为 60000 字节。加载失败保留目标并提供重试。

MD 正文清空迁到编辑工具栏，统一确认并使用原生撤销，不保留编辑区重复清空入口。任务“新增”输入支持多行纯文本、-／* 列表和 - [ ]／- [x] 格式，按非空有效行追加独立 ID；相同文本不去重。一次最多满足现有 1,000 项任务、单项 8,192 字节限制，超限整体拒绝；新任务可共用负责人／截止时间。提供本次新增撤销，原任务已修改时禁用，避免丢失后续内容。编辑已有任务正文不会自动拆分。

本轮实现已通过 383 项工作区测试（1 项忽略）、全目标 Clippy 与格式检查。Windows 隔离 SQLite 原生窗口已验证：连接失败后在提示列表重试并直接选列、FROM 前与未完成 CASE 的列提示、不同宽度双视图的紧凑工具栏、DDL 当前表关联、SQL文件显式范围／恢复继承且不触发执行、旧结果保留、MD 清空及撤销、任务批量新增／勾选状态／撤销。Debug 与 Release 已重编译；这不代表真实 MySQL/PostgreSQL 服务端、Linux 桌面或真实输入法组词已验收，未打包发布。

## 函数说明与列标识

函数候选显示名称和简短用途，上下键切换时就地显示选中项的调用形式、参数和示例；内置函数按当前驱动提供离线资料，关键词仅显示一行用途。帮助卡支持插入调用模板、复制示例及按需展开更多示例；窄栏将卡片放在列表下方，长内容可滚动，靠近底部时优先在上方展示。

Enter／鼠标候选插入函数名，Tab／模板按钮插入调用并选中首个参数。已存在括号及参数时保留原内容（也识别括号前注释），不重复追加括号；一次原生撤销可还原模板插入，输入法组词不接受补全。帮助与示例仅辅助编辑，复制及插入均不执行 SQL。不在逐键路径联网查询函数文档或读取远程元数据。

列候选与表头共用渲染：主键黄色钥匙优先，文本蓝色 ABC，数值青绿色 123，日期时间紫色日历，未知类型中性标识。实际列类型和主键信息取当前范围缓存；插入文本只有列名，长类型／注释可悬停查看。

内置资料是常用语法参考，不代表当前服务端版本和扩展均已支持。示例依据官方参考核对：[MySQL 日期](https://dev.mysql.com/doc/refman/8.4/en/date-and-time-functions.html)、[MySQL 字符串](https://dev.mysql.com/doc/refman/8.4/en/string-functions.html)、[PostgreSQL 格式化](https://www.postgresql.org/docs/current/functions-formatting.html)、[SQLite 日期](https://www.sqlite.org/lang_datefunc.html)、[SQLite 聚合](https://www.sqlite.org/lang_aggfunc.html)；版本限制保留在相应函数帮助中。

本轮通过 385 项工作区测试（1 项忽略）、全目标 Clippy、格式及双语占位符检查。Windows 隔离窗口覆盖函数名／模板接受、首参数替换、按钮插入、原生撤销、已有参数保留、窄栏更多示例、底部反向定位与 Unicode 范围，以及主键／文本／数值／日期标识；补全接受未产生查询执行记录。23 条 SQLite 帮助示例另在本机隔离内存库执行通过。MySQL 日期帮助采用本地受控连接桩验证离线显示，未连接真实 MySQL/PostgreSQL/ClickHouse 验收函数执行。本轮更新 target/debug/turbodbnote.exe，未重编译 Release，未发布。

选区优先 DDL 修复已通过离线解析回归和 Windows 隔离 SQLite 原生窗口验证：选中引用表名、完整语句、限定名称、多表候选、字符串排除及缺失表定义提示；DDL 操作未执行编辑器 SQL，也未连接无关服务。此次更新 Debug；不代表真实 MySQL/PostgreSQL 服务端或 Linux 桌面验收。

Windows 安装版更新由安装目录外的独立助手处理：下载／SHA256 校验显示真实进度，保存并确认后等待主进程和其他实例退出。安装向导继续展示真实安装进度，完成后可选重开且不执行 SQL。欢迎／完成页左侧展示应用 Logo，其余页面标题左侧展示相同小 Logo，助手也复用应用图标。旧版本需先手动安装修复版一次；绿色版不自动覆盖。

独立更新助手本轮验证：393 项工作区测试通过（1 项忽略），全目标检查／Clippy 与格式检查通过；Windows 无害模拟安装器覆盖完整／精简包、可替换已退出程序、成功后重开／不重开、其他实例阻止、并发助手拒绝、取消、错误哈希、包身份／目录／父进程拒绝，共 10 个原生场景。安装向导品牌界面使用独立 AppId 的隔离夹具验证，仅查看欢迎和许可页面，未执行真实升级安装；主程序和助手图标检查及隔离启动通过。尚未打包发布，也不代表 Linux 桌面或真实用户安装升级已验收。
