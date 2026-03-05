# PNGOptim 项目阶段记忆（AGENTS）

> 最后更新：2026-03-05  
> 依据文档：`docs/RECONSTRUCTION_MASTER_PLAN.md`、`docs/PHASE_TASKLIST.md`、`docs/EVALUATION_SPEC.md`、`docs/ACCEPTANCE_MATRIX.md`

## 1. 项目开发目的（统一认知）

本项目目标是用 Rust 在单仓库内实现一个可发布的 PNG 量化压缩工具，并按“先复刻、后优化”的策略达到或超过对标工具（pngquant/libimagequant）的工程能力：

1. 功能与 CLI 语义高兼容（参数、退出码、I/O、元数据策略）。
2. 体积、质量、性能达到统计意义等价或更优。
3. 跨平台稳定（macOS / Linux / Windows）且可复现。
4. 全过程以可自动化评测和报告驱动，不做无数据优化。

当前明确非目标：
1. 不追求 bit-exact 输出。
2. 不做 GUI。
3. 不扩展 PNG 之外格式。
4. v1 不引入复杂 ML 压缩策略。

## 2. 分阶段开发规划（不设周期，仅阶段）

执行顺序固定：A -> B -> C -> D -> E -> F -> G

### 阶段 A：治理与基线先行
- 目标：先定规则与基线，再进入实现。
- 关键任务：
1. 冻结主方案版本与文档先行机制。
2. 建立依赖准入与许可证审查流程。
3. 建立评测数据集、参数矩阵、报告规范。
4. 产出并冻结 baseline 报告。
- 阶段出口：`Compliance Policy v1` + `Baseline Report v1` + `Acceptance Matrix v1`
- 当前状态：`Done`

### 阶段 B：最小可运行闭环
- 目标：端到端可跑（读 -> 量化 -> 写），且基础稳定。
- 关键任务：
1. 打通最小 pipeline。
2. 完成 CLI 初版（单文件、基础参数、错误码框架）。
3. 全样本 smoke 测试通过且无崩溃。
- 阶段出口：`MVP Pipeline` + `Smoke Report v1`
- 当前状态：`Done`

### 阶段 C：行为语义复刻
- 目标：工具使用语义与对标工具一致。
- 关键任务：
1. 参数语义对齐（quality/speed/dither/output/ext/strip/skip-if-larger/posterize）。
2. 退出码与错误语义对齐。
3. stdin/stdout、批处理、覆盖策略对齐。
4. 元数据策略对齐。
- 阶段出口：`Compatibility Report v1` + 行为差异清单
- 当前状态：`Done`

### 阶段 D：质量与体积复刻
- 目标：核心压缩能力达到对标水平。
- 关键任务：
1. 质量指标达标（SSIM/Butteraugli/PSNR 组合门禁）。
2. 体积指标达标（均值/中位数/P95）。
3. 专项场景修复（低色、透明边缘、UI/渐变）。
4. 失败样本闭环清理（持续下降或清零）。
- 阶段出口：`Quality & Size Report v1`
- 当前状态：`Done`

### 阶段 E：性能优化冲刺
- 目标：形成可量化性能优势。
- 关键任务：
1. 模块级耗时与内存可观测。
2. 搜索/抖动/写出热点逐项优化。
3. 引入 SIMD 与并行调度策略。
4. 每次优化必须回归质量与体积门禁。
- 阶段出口：`Perf Report v1` + 资源画像报告
- 当前状态：`Done`

### 阶段 F：稳定性与跨平台收口
- 目标：达到发布候选质量。
- 关键任务：
1. 回归 + fuzz 零崩溃。
2. 三平台一致性回归（macOS/Linux/Windows）。
3. 可复现构建与 RC 门禁落地。
- 阶段出口：`RC Candidate` + `Stability Report v1` + `Cross-platform Report v1`
- 当前状态：`In Progress`

### 阶段 G：开源发布与社区协作
- 目标：具备可持续开源协作能力。
- 关键任务：
1. 发布文档、评测脚本、样本说明、许可证声明。
2. 建立贡献规范、Issue/PR 模板和回归流程。
3. 建立长期性能回归与样本扩充机制。
- 阶段出口：`Public Release v1`
- 当前状态：`Blocked`

## 3. 验收门禁（阶段推进依据）

1. `MVP` 门槛：DOD-01/02/03/10/12 通过。
2. `复刻` 门槛：MVP + DOD-05/06/07 通过。
3. `发布候选` 门槛：复刻 + DOD-08/09/11 通过。
4. 规则：任一 P0 失败不可合并；P1 豁免必须登记风险和关闭条件。

## 4. 进度记录（持续更新区）

### 阶段状态总览
| 阶段 | 状态 | 当前焦点 | 证据/报告 |
|---|---|---|---|
| A | Done | 阶段收口完成 | `docs/phase-a/PHASE_A_PROGRESS.md` |
| B | Done | 阶段收口完成 | `docs/phase-b/PHASE_B_PROGRESS.md` |
| C | Done | 阶段收口完成 | `docs/phase-c/PHASE_C_PROGRESS.md` |
| D | Done | 阶段收口完成 | `docs/phase-d/PHASE_D_PROGRESS.md` |
| E | Done | 阶段收口完成 | `docs/phase-e/PHASE_E_PROGRESS.md` |
| F | In Progress | 三平台一致性收口 | `docs/phase-f/PHASE_F_PROGRESS.md` |
| G | Blocked | 依赖 F 三平台收口 | `docs/phase-g/PHASE_G_PROGRESS.md` |

### 最近更新
1. 2026-03-05：确认参考仓库本地路径与远程可达性，并锁定 `main` 分支 commit。
2. 2026-03-05：新增 `Compliance Policy v1`、依赖登记模板、参数矩阵、数据集目录骨架。
3. 2026-03-05：新增 `Baseline Report v1`（源锁定与环境快照），阶段 A 进入 `In Progress`。
4. 2026-03-05：导入首批功能样本（2 个）并建立 `manifest.json`，用于后续 baseline 跑数。
5. 2026-03-05：新增 baseline 跑数脚本并完成首轮 `Q_MED` 跑数（functional 2/2 成功）。
6. 2026-03-05：完成文档变更流程（Doc Change Process + PR 模板）与 CI 合规门禁（cargo-deny workflow）。
7. 2026-03-05：补齐 quality/perf/robustness 数据集样本，并完成 baseline v1 全量跑数（unexpected=0）。
8. 2026-03-05：阶段 A 出口条件达成，状态更新为 `Done`。
9. 2026-03-05：完成阶段 B MVP 代码（读取/量化/写出 + CLI + 错误码框架）。
10. 2026-03-05：执行全样本 smoke（9/9 通过，无崩溃），阶段 B 更新为 `Done`。
11. 2026-03-05：完成阶段 C 参数/退出码/I/O 兼容性验证（`compat-v1-20260305`）。
12. 2026-03-05：完成 metadata preserve/strip 行为实现并通过兼容性验证，阶段 C 更新为 `Done`。
13. 2026-03-05：完成 indexed PNG 编码优化（位深自适应、过滤器择优、透明表裁剪），修复阶段 D 高回归样本。
14. 2026-03-05：完成阶段 D 质量/体积评测（`quality-size-v1-20260305-r3`），均值/中位数/P95 均优于 baseline。
15. 2026-03-05：完成回归守护验证（`smoke-v1-20260305-d-encoding` + `compat-v1-20260305-d-encoding`），阶段 D 更新为 `Done`。
16. 2026-03-05：完成 Phase E 评测命令（`cargo run --release --bin xtask -- perf`），支持 `perf_compare.csv` 与 `memory_profile.json` 产出。
17. 2026-03-05：新增模块级耗时埋点与量化/编码热点优化，完成 release 性能评测（`perf-v1-20260305-e5`）。
18. 2026-03-05：完成阶段 E 回归守护（`smoke-v1-20260305-e`、`compat-v1-20260305-e`、`quality-size-v1-20260305-e-guard-r3`），阶段 E 更新为 `Done`。
19. 2026-03-05：新增 Phase F 稳定性命令（`cargo run --release --bin xtask -- stability`），完成 `stability-v1-20260305-f1`（49 case，0 crash/panic/timeout）。
20. 2026-03-05：新增 Phase F 跨平台命令（collect+aggregate）与 CI 工作流（`.github/workflows/phase-f-cross-platform.yml`）。
21. 2026-03-05：完成本地跨平台链路验证（`cross-platform-v1-20260305-f1` partial），阶段 F 更新为 `In Progress`，待 CI 三平台收口。
22. 2026-03-05：新增阶段 G 协作资产（`CONTRIBUTING.md` + Issue 模板 + `nightly-regression` workflow）。
23. 2026-03-05：新增发布资产命令（`cargo run --release --bin xtask -- release-licenses`、`cargo run --release --bin xtask -- release-check`）。
24. 2026-03-05：新增阶段 G 预检文档（`docs/phase-g/PUBLIC_RELEASE_V1.md`），阶段 G 状态标记为 `Blocked`（等待 F 收口）。
25. 2026-03-05：将 Phase F 跨平台 CI 编排迁移为 Rust `xtask`（`src/bin/xtask.rs`），`phase-f-cross-platform` workflow 已改为 `cargo run --bin xtask`，不再依赖 Python 运行时。
26. 2026-03-06：将 `nightly-regression` workflow 迁移为 Rust `xtask nightly-regression`，主 CI 编排链路不再要求 Python 环境。

### 更新规则
1. 每次推进必须更新对应阶段状态：`Not Started` / `In Progress` / `Blocked` / `Done`。
2. 每次推进至少记录一条证据路径（报告或测试输出）。
3. 每次提交任务需绑定 DoD 条款编号（例如 `DOD-05`）。
4. 不做无数据优化；无法量化收益的改动不进入主线。
