# PNGOptim 项目阶段记忆（AGENTS）

> 最后更新：2026-03-06  
> 依据文档：`docs/RECONSTRUCTION_MASTER_PLAN.md`、`docs/PHASE_TASKLIST.md`、`docs/EVALUATION_SPEC.md`、`docs/ACCEPTANCE_MATRIX.md`

## 1. 项目开发目的（统一认知）

本项目目标是用 Rust 在单仓库内实现一个可发布的 PNG 量化压缩工具，并按“先复刻、后优化”的策略达到或超过对标工具（pngquant/libimagequant）的工程能力：

1. 功能与 CLI 语义高兼容（参数、退出码、I/O、元数据策略）。
2. 体积、质量、性能达到统计意义等价或更优。
3. 跨平台稳定（macOS / Linux / Windows）且可复现。
4. 全过程以可自动化评测和报告驱动，不做无数据优化。
5. 在核心量化算法上优先对齐 `pngquant` / `libimagequant` 的实现思路：允许编码实现不同，但 `quality`、palette search、remap、dither 的主链应一致。

当前明确非目标：
1. 不追求 bit-exact 输出。
2. 不做 GUI。
3. 不扩展 PNG 之外格式。
4. v1 不引入复杂 ML 压缩策略。

## 1.1 工程约束与决策锁定

1. 仓库主线、CI 编排、发布资产生成统一使用 Rust；不得把 Python 重新引入为项目运行时或主线编排依赖。
2. 临时本地分析命令不视为项目资产；若使用一次性 shell/Python 片段做实验，禁止提交进仓库，也不得让文档、workflow、CLI 依赖该运行时。
3. 算法目标不是“完全自己发明一套”，而是尽可能对齐 `pngquant` / `libimagequant` 的成熟实现思路。
4. 目前不直接复制或链接参考实现代码进入主线，不是出于教条式自研，而是因为当前项目已按 MIT 发布，直接引入参考实现代码需要先明确许可证策略与仓库治理方案。
5. 如果后续决定直接复用参考实现代码，必须先把许可证、分发方式、仓库结构和发布策略记录进文档，再执行代码接入。
6. 后续所有算法复刻工作采用 `reference-first` 纪律：先对照本地参考仓库对应模块提炼实现细节、启发式和边界条件，再编码；禁止脱离参考实现结构凭感觉连续打补丁。

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
- 当前状态：`Done`

### 阶段 G：开源发布与社区协作
- 目标：具备可持续开源协作能力。
- 关键任务：
1. 发布文档、评测脚本、样本说明、许可证声明。
2. 建立贡献规范、Issue/PR 模板和回归流程。
3. 建立长期性能回归与样本扩充机制。
- 阶段出口：`Public Release v1`
- 当前状态：`Done`

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
| F | Done | 阶段收口完成 | `docs/phase-f/PHASE_F_PROGRESS.md` |
| G | Done | 阶段收口完成 | `docs/phase-g/PHASE_G_PROGRESS.md` |

### 附加产品轨道
| 轨道 | 状态 | 当前焦点 | 证据/报告 |
|---|---|---|---|
| Algorithm Replication | In Progress | 对齐 `pngquant/libimagequant` 的核心量化主链 | `docs/phase-d/ALGORITHM_REPLICATION_ANALYSIS_V1.md` |

### Algorithm Replication 新规划（Reference-First）
| 子阶段 | 状态 | 参考模块 | 目标 | 当前结论 |
|---|---|---|---|---|
| RF-1 | Done | `pngquant.c` + `attr.rs` | 对齐 `quality/speed` 语义、预算和门禁标尺 | `quality <-> MSE` 已接通 |
| RF-2 | Partially Done | `quant.rs` + `mediancut.rs` + `kmeans.rs` | 对齐 feedback loop、palette search、unused color replacement | 已有骨架，但误差约束收缩仍不稳定 |
| RF-3 | Done | `nearest.rs` | 对齐 VP-tree nearest、likely-index、剪枝逻辑 | 已完成，性能回退已大幅收回 |
| RF-4 | Partially Done | `remap.rs::remap_to_palette` | 对齐 remap 阶段 palette 统计回灌、background/importance 处理 | plain remap 反馈已接入，background/importance 与误差口径仍缺 |
| RF-5 | Partially Done | `remap.rs::dither_map` + `remap_to_palette_floyd` | 对齐 dither map、selective Floyd、background-aware 分支 | core subset 已接入，background-aware 与质量收益仍不足 |
| RF-6 | Partially Done | `pngquant.c` + `quant.rs` | 对齐 `skip-if-larger` 启发式和 remap 后质量决策 | same-score 候选已按更小输出优先，完整启发式仍缺 |
| RF-7 | Pending | 全链路 | 重跑 quality/perf/stability/release 门禁，形成新基线 | 在 RF-4/5 完成后执行 |

### 当前硬阻塞与下一步
1. 当前主线已把 `demo.png` 的默认输出质量从 `quality_score=45` 提升到 `56`，最近一次 RF-4 spot check 为 `quality_mse=14.463`, `131912 bytes`；但在 `--quality 65-75` 下仍只有 `actual=57`，尚未满足最低质量门槛。
2. naive 全图 Floyd 已被证明方向错误：会放大输出并拉低质量；当前已接入 `dither map + selective Floyd` 的核心子集，并新增 same-score 候选的 size-aware 决策，但 `demo.png` 默认输出仍约 `131919 bytes`、`quality_score=56`，说明主要缺口仍在质量路径而不是单纯候选选择。
3. `Nearest` 结构已按 `libimagequant/src/nearest.rs` 的 VP-tree 思路接入主线，原先的 perf 大回退已显著收回；当前主要缺口重新回到质量与 remap/dither 主链，而不是 nearest search。
4. 当前最合理的执行顺序已经调整为：`RF-4 importance/remap error 收口` -> `RF-5 background-aware / dither decision 收口` -> `RF-6 skip-if-larger 启发式收口` -> `RF-7 全门禁回归`。

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
27. 2026-03-06：Phase F 三平台 CI 收口完成（`phase-f-cross-platform` run `22722936354` 全绿），新增 `RC Candidate v1` 并将阶段 F 标记为 `Done`，阶段 G 由 `Blocked` 切换为 `In Progress`。
28. 2026-03-06：新增发布打包命令 `xtask release-package`，补齐 `LICENSE`、`USER_GUIDE_V1`、`BENCHMARK_REPRO_V1`，并产出发布包 `public-release-v1-20260306-g1-verify`（22 个文件，含 SHA256 清单）。
29. 2026-03-06：新增 `xtask ci-trends` 与 `ci-trend-dashboard` workflow，生成趋势报告 `ci-trends-v1-20260306`，阶段 G 最后一个缺口收口，状态更新为 `Done`。
30. 2026-03-06：刷新最终发布包为 `public-release-v1-20260306-g2-verify`，将趋势看板文档与 workflow 一并纳入发布清单（24 个文件）。
31. 2026-03-06：完成 `pngquant` / `libimagequant` 深度实现分析，确认当前实现与参考主链在 `quality` 语义、histogram、palette search、remap、dither 上存在架构级差距，新增 `docs/phase-d/ALGORITHM_REPLICATION_ANALYSIS_V1.md` 并启动 `Algorithm Replication` 附加产品轨道。
32. 2026-03-06：启动 R1，已将 `--quality` 解析扩展为 `N / -N / N- / min-max`，引入 libimagequant 风格 `quality <-> MSE` 标尺、speed 策略骨架与基于质量目标的最小色数搜索桥接实现，为后续 R2 的 palette search 重写清理接口。
33. 2026-03-06：执行 R1 回归验证：`compat` 通过（`reports/compat/r1-compat-verify/summary.md`），但 `smoke` 在新质量门禁下仅 `2/9` 通过（`reports/smoke/r1-smoke-verify/summary.md`），确认当前旧量化器在真实质量标尺下已不能满足既有阶段 D 结论，需进入 R2 重写核心 palette search / remap。
34. 2026-03-06：完成 R2 第一版自研量化器替换（gamma-aware histogram + weighted median cut + k-means refine + palette prune/remap），`compat` 通过（`reports/compat/r2-compat-verify/summary.md`），`smoke` 恢复到 `9/9` 通过（`reports/smoke/r2-smoke-verify/summary.md`）；但 perf 样本耗时显著上升，后续需回到阶段 E 做性能收口。
35. 2026-03-06：验证了 naive 全图 Floyd remap：在 `demo.png` 上会把输出放大到约 `348KB` 且质量分数下降到 `22`，不符合 pngquant 的 selective dithering 思路；当前仅在启用抖动时做“择优采用”，后续需实现 dither map/选择性抖动而非全图误差扩散。
36. 2026-03-06：完成阶段记忆审计，确认仓库中不存在 `.py` 文件、workflow 也未重新依赖 Python；此前出现的 Python 仅为一次性本地分析命令，未进入主线。
37. 2026-03-06：将“Rust-only 主线编排”和“参考实现可借鉴但受许可证约束”的决策写入阶段记忆，避免后续在实现路线与依赖策略上反复横跳。
38. 2026-03-06：完成 R2.1 第一版反馈式 palette search：将 `target_mse` 与 `feedback_loop_trials` 接入 Rust quantizer，并把 histogram 权重反馈到后续 median cut；`compat` 通过（`reports/compat/r2-1d-compat-verify/summary.md`），`smoke` 通过（`reports/smoke/r2-1d-smoke-verify/summary.md`）。
39. 2026-03-06：为中等复杂度直方图新增 1 次真实像素 remap 收敛，`demo.png` 默认输出提升到 `quality_score=56`, `quality_mse=14.644`, `131278 bytes`；但 `--quality 65-75` 仍失败（`actual=57`），且 `perf-002-large-alpha-pattern` 仍为当前主要性能回退样本（`104049.867 ms`）。
40. 2026-03-06：确认算法推进方法调整为 `reference-first`：后续 R2/R3 只按 `pngquant.c`、`libimagequant/src/attr.rs`、`quant.rs`、`mediancut.rs`、`kmeans.rs`、`nearest.rs`、`remap.rs` 的对应模块逐段复刻，不再以“自拟近似方案”为主。
41. 2026-03-06：完成 R2.2 第一步 `nearest.rs` 对齐：引入 VP-tree 风格 nearest search、likely-index 提前命中和 nearest-other-color 距离剪枝，并切换 kmeans/remap/dither/pixel-refine 全部调用点；`compat` 通过（`reports/compat/r2-2-compat-verify/summary.md`），`smoke` 通过（`reports/smoke/r2-2-smoke-verify/summary.md`）。
42. 2026-03-06：`Nearest` 对齐后，perf 样本显著恢复：`perf-001-large-gradient-noise` 从 `35490.128 ms` 降到 `5085.685 ms`，`perf-002-large-alpha-pattern` 从 `104049.867 ms` 降到 `11901.303 ms`；`demo.png` 质量保持在 `quality_score=56/57`，说明下一步瓶颈已转移到 `remap.rs` / selective dithering。
43. 2026-03-06：对算法复刻轨道重新规划，废弃过粗的 `R1/R2/R3` 执行粒度，改为 `RF-1 .. RF-7` 的 reference-first 模块计划；后续不再按“先写近似实现再逐步修正”的方式推进。
44. 2026-03-06：完成 RF-4 第一段 `remap_to_palette` 对齐：plain remap 已改为真实像素统计回灌 + 最终 remap 两段式收敛，`compat` 通过（`reports/compat/rf4-compat-verify/summary.md`），`smoke` 通过（`reports/smoke/rf4-smoke-verify/summary.md`）；但 `demo.png` 仅提升到 `quality_mse=14.463`, `131912 bytes`，`--quality 65-75` 仍失败（`actual=57`），说明下一主攻点已转到 `RF-5 selective dithering`。
45. 2026-03-06：完成 RF-5 核心子集：按 `image.rs` / `remap.rs` 思路接入 contrast-map 驱动的 selective Floyd、serpentine 扫描、`max_dither_error` 限制和 remapped guess；`compat` 通过（`reports/compat/rf5-compat-verify/summary.md`），`smoke` 通过（`reports/smoke/rf5-smoke-verify/summary.md`），perf 样本为 `5886.829 ms` / `15419.006 ms`。但 `demo.png` 默认输出仍为约 `131920 bytes`, `quality_score=56`，`--quality 65-75` 仍失败（`actual=57`），说明 RF-5 仍需继续补 background-aware 分支和更完整的 dither 决策。
46. 2026-03-06：启动 RF-6 第一段决策层对齐：同一色数下 plain/dither 候选在相同 `quality_score` 且 MSE 接近时，改为优先保留更小输出的候选；`compat` 通过（`reports/compat/rf6-compat-verify/summary.md`），`smoke` 通过（`reports/smoke/rf6-smoke-verify/summary.md`），`q_gradient_photo_like` 进一步降到 `327981 bytes`（`quality_score=70`, `quality_mse=9.215`）。但 `demo.png` 仍保持在 `131919 bytes`, `quality_score=56`，说明 RF-6 只能改善候选选择，无法替代质量主链收口。

### 更新规则
1. 每次推进必须更新对应阶段状态：`Not Started` / `In Progress` / `Blocked` / `Done`。
2. 每次推进至少记录一条证据路径（报告或测试输出）。
3. 每次提交任务需绑定 DoD 条款编号（例如 `DOD-05`）。
4. 不做无数据优化；无法量化收益的改动不进入主线。
