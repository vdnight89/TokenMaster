# TokenMaster 文档库索引

本目录是项目的核心文档库，开发循环（[dev-loop.md](./dev-loop.md)）围绕它运转：**任务从 TASKS.md 领取，实现对照 spec 与 reference 手册，架构决策落 ADR，执行轨迹记 dev-log**。

## 主线（按阅读顺序）

| 文档 | 作用 | 消费时机 |
|---|---|---|
| [../CONTEXT.md](../CONTEXT.md) | 领域术语表（Provider/账号/令牌池/网关/模型路由…） | 写代码与测试时统一命名 |
| [adr/](./adr/) | 架构决策记录 0001-0007（技术栈/许可/网关契约/功能边界…） | 动架构前先查，避免推翻已决 |
| [spec/v1.md](./spec/v1.md) | **v1 规格说明**：35 条用户故事 + 实现决策 + 测试决策（双缝）+ 范围外 | 每个任务的行为基准与验收来源 |
| [TASKS.md](./TASKS.md) | 任务板（M0-M7 全量拆解 + 里程碑集成关卡） | 领任务/记录状态 |
| [dev-loop.md](./dev-loop.md) | 开发循环协议（TDD 红→绿→自查→关卡→文档→提交） | 每个任务执行时 |
| [dev-log.md](./dev-log.md) | 开发日志（每任务一行审计轨迹） | 每任务结束时追加 |

## 参考手册（实现时对照，全部带文件路径与行号）

| 手册 | 内容 |
|---|---|
| [reference/deepseek-harness-codearts.md](./reference/deepseek-harness-codearts.md) | 14 家 provider 逐家协议对照（endpoint/登录/SSE/坑）——Rust 重写的对照基准 |
| [reference/antigravity-manager.md](./reference/antigravity-manager.md) | 可直接拷贝的 Rust 模块（token_manager/oauth/重试换号）施工地图 |
| [reference/commandcode-proxy.md](./reference/commandcode-proxy.md) | commandcode 私有协议 + JS→Rust 移植对照表 |
| [reference/zcode-pool.md](./reference/zcode-pool.md) | enc:v1 凭据格式（导入功能关键）、GUI 设计蓝本、i18n 方案 |

## 设计资产

| 资产 | 内容 |
|---|---|
| [../design/round-02/](../design/round-02/) | **UI 实现基准**：AM 式暗色控制台原型（7 页/11 图表/完整交互），design-notes.md 为设计说明；原型源码（app.js/app.css/index.html）已移植完成后归档为 `prototype.zip`，页面截图在 `shots/` |
| [../design/round-01/](../design/round-01/) | 方向探索存档（对比页 + 三方向，已被 round-02 取代） |

## 约定

- 文档只增不删：决策变更用修订节或新 ADR，不抹历史。
- 术语以 CONTEXT.md 为准，与代码标识符保持一致。
- 新增文档先进本索引。
