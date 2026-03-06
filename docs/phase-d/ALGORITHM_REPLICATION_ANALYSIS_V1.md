# Algorithm Replication Analysis v1

> 阶段：D（质量与体积复刻，算法深化轨道）  
> 更新日期：2026-03-06  
> 目标：将当前实现从“结果上近似可用”推进到“算法思路与 `pngquant` / `libimagequant` 一致”

## 1. 参考实现范围

- `pngquant` 本地参考仓库：`/Users/5km/Dev/C/pngquant`
- `pngquant` 锁定 commit：`e3bdc7c9b8b814409555a7aa30adcfb3997fa115`
- `libimagequant` 本地参考仓库：`/Users/5km/Dev/C/libimagequant`
- `libimagequant` 锁定 commit：`b1df2d2715521b9a34173314755aee851449b1c4`

本轮分析只聚焦“算法与处理链路”，不要求位级输出一致，也不要求编码器内部实现一致。

## 1.1 实施约束

1. 项目主线、CI 编排和发布链路保持 Rust-only；不把 Python 重新引入为仓库运行时依赖。
2. 允许参考 `pngquant` / `libimagequant` 的工程实现思路，目标是复刻成熟算法，而不是为“纯自研”牺牲结果。
3. 当前不直接复制或链接参考实现代码进入 MIT 主线，原因是许可证策略尚未调整；这属于许可证与分发治理问题，不是技术路线上的教条限制。
4. 如果后续决定直接复用参考实现代码，必须先在仓库文档中记录许可证、分发、仓库结构和发布策略变更。

## 1.2 Reference-First 执行顺序

后续算法工作按下面顺序推进，先读参考实现，再编码：

1. `pngquant.c`
   - 关注 CLI 语义、`quality`/`skip-if-larger`/I/O/metadata 的入口决策。
2. `libimagequant/src/attr.rs`
   - 关注 `speed`、`feedback_loop_trials`、`kmeans_iterations`、posterization、dither-map 开关。
3. `libimagequant/src/quant.rs`
   - 关注 `find_best_palette()`、`target_mse/max_mse`、feedback loop、最终 refine 退出条件。
4. `libimagequant/src/mediancut.rs`
   - 关注 box split 优先级、误差约束、颜色权重。
5. `libimagequant/src/kmeans.rs`
   - 关注 remap 后统计回灌、unused color replacement、权重调整。
6. `libimagequant/src/nearest.rs`
   - 关注 VP-tree nearest search。
7. `libimagequant/src/remap.rs`
   - 关注 remap 阶段 palette 再收敛、dither map、selective Floyd、background-aware 分支。

约束：

1. 每个子阶段至少要能指出“参考实现的对应模块 + 我们当前缺口 + 本轮只补哪一块”。
2. 没有完成对应参考模块阅读前，不进入编码。
3. 任何明显偏离参考结构的实现，都必须先说明偏离原因和预期收益。

## 2. 参考实现职责边界

### 2.1 `pngquant` 负责什么

`pngquant` 负责的是 CLI 与文件级编排，不是核心量化算法本身。主职责包括：

1. 解析 CLI 参数和退出码语义。
2. 读取 PNG、处理 gamma / ICC / metadata。
3. 调用 `libimagequant` 生成量化结果与重映射索引。
4. 根据 `--skip-if-larger`、`stdout`、覆盖策略做最终决策。
5. 将调色板图像编码回 PNG。

关键入口：

- `pngquant.c::parse_quality`
- `pngquant.c::pngquant_main_internal`
- `pngquant.c::pngquant_file_internal`

### 2.2 `libimagequant` 负责什么

`libimagequant` 负责的是核心量化链路：

1. 将 `quality` 语义映射到误差阈值（MSE）。
2. 构建直方图与感知权重。
3. 通过 mediancut + feedback loop + k-means 寻找最优调色板。
4. 通过最近邻搜索进行 remap。
5. 通过 dither map + selective Floyd dithering 做最终重映射。

关键模块：

- `src/attr.rs`
- `src/hist.rs`
- `src/mediancut.rs`
- `src/kmeans.rs`
- `src/nearest.rs`
- `src/quant.rs`
- `src/remap.rs`

## 3. 参考实现的核心语义

### 3.1 `--quality` 不是“取中值”

`pngquant` 的 `--quality` 语义是：

1. `min` 是最低可接受质量。
2. `max` 是目标质量上界。
3. 算法会搜索“满足或超过 `max` 的最少颜色数”。
4. 如果最终质量低于 `min`，则返回 `QUALITY_TOO_LOW`。

`pngquant.c::parse_quality` 只是做字符串解析；真正的质量含义由 `libimagequant::Attributes::set_quality()` 定义。

在 `libimagequant` 中：

1. `set_quality(min, max)` 会把质量转换成 `max_mse` 与 `target_mse`。
2. `quality_to_mse()` / `mse_to_quality()` 是质量标尺的核心。
3. 质量搜索是“误差约束下找更小 palette”，不是“quality -> 固定色数”。

这与当前项目的实现有根本差异。当前实现把 `65-75` 直接取中值 `70`，再映射为 `max_colors`，这不是 pngquant 语义。

## 4. 参考实现的算法主链

### 4.1 直方图与感知空间

`libimagequant` 不是直接在原始 `u8 RGBA` 上做频率截断。

它在 `hist.rs` 中做了几件关键事情：

1. 将颜色放入带 gamma 处理的浮点感知空间。
2. 使用 `importance_map` / contrast map 调整权重。
3. 对过大的直方图自动提高输入 posterization 位数，控制状态空间。
4. 将 fixed colors 作为高权重颜色注入直方图。
5. 先用 16 个 cluster 做初始分组，避免极端颜色被平均掉。

这意味着它优化的是“感知误差”，不是“原始 RGBA 的简单平均差值”。

### 4.2 调色板搜索不是简单截断

`find_best_palette()` 的实际流程是：

1. 先根据 `target_mse`、`max_mse`、`max_colors` 启动搜索。
2. 用 `mediancut()` 生成候选 palette。
3. 对候选 palette 执行 `Kmeans::iteration()`。
4. 如果误差更低，或者在满足质量约束下颜色更少，则保留该候选。
5. 继续 feedback loop，动态缩小 `max_colors`，反复搜索更优解。
6. 最后再执行 `refine_palette()` 做额外 k-means 收敛。

这条链路的目标不是“尽量多保留高频色”，而是“在误差约束下找到更小且更稳的 palette”。

### 4.3 mediancut 是带误差约束的

`mediancut.rs` 里不是传统教科书式的“按最大方差劈半然后结束”。

它的关键特征：

1. box 的 split 优先级受方差、权重、最大误差共同影响。
2. `take_best_splittable_box()` 会优先切分最值得切的 box。
3. 在切分过程中会检查整体 `target_mse` 是否已达标。
4. 会根据 `max_mse_per_color` 控制单个颜色桶的误差上限。

所以 palette 大小和分布是搜索出来的，不是一次性拍板。

### 4.4 k-means 不只是“锦上添花”

`kmeans.rs` 有两个关键作用：

1. 让 palette 向局部最优移动，而不是停在 mediancut 的代表色上。
2. 对 remap 之后“无人使用”的 palette entry 进行替换，避免冗余颜色位。

这对体积与质量都重要。冗余 palette entry 会直接拉高 palette 大小、索引熵和最终 IDAT 体积。

### 4.5 remap 不是线性暴力最近邻

`nearest.rs` 构建的是 VP-tree（vantage-point tree），不是简单 `O(N_palette)` 暴力扫描。

用途有两层：

1. 加速 palette nearest search。
2. 为 remap 和 dithering 提供更稳定的最近色选择。

而 `remap_to_palette()` 还会在 remap 阶段继续累计 k-means 统计，对最终 palette 进一步微调。

### 4.6 dithering 是“选择性 Floyd”，不是单开关

`remap.rs` 的 dithering 路径有几个当前项目完全缺失的关键点：

1. 先生成 dither map，而不是直接全图 Floyd。
2. dither map 会尽量避开边缘和高噪声区，只在平坦区域增强视觉平滑。
3. 使用 serpentine 扫描和误差扩散。
4. 有 `max_dither_error` 上限，防止错误扩散过头。
5. 有 background-aware 分支，避免透明背景和静态背景区域出现抖动伪影。
6. 采用分块并行，并对 chunk 起始行做预热，减少并行缝线。

这不是一个布尔选项，而是一条完整的重映射算法。

## 5. `speed` 在参考实现中的真实作用

在 `libimagequant::Attributes::set_speed()` 中，`speed` 会同时影响：

1. k-means 迭代次数与迭代停止阈值。
2. feedback loop 次数。
3. 允许的 histogram 大小。
4. 输入 posterization。
5. 是否启用 dither map。
6. 是否启用 contrast map。
7. 是否单线程 dithering。

因此 `speed` 不是简单的“编码快一点/慢一点”，而是整条量化链路的搜索预算与策略开关。

## 6. `skip-if-larger` 在参考实现中的真实语义

`pngquant` 的 `--skip-if-larger` 也不是单纯的“输出大于输入则失败”。

它使用一个质量和尺寸联动的启发式：

1. 根据 remap 后质量估计 `quality_percent`。
2. 计算 `expected_reduced_size = quality^(1.5)`。
3. 用这个值估算“这次质量损失至少应该换来多少体积收益”。

也就是说，`pngquant` 会把“尺寸收益是否值得质量损失”一起算进去。

## 7. 当前项目与参考实现的关键差距（基于 R2.1 当前状态）

以下差距不是调参问题，而是架构级差距：

### 7.1 质量语义已接通，但质量搜索仍未闭环

- 当前：R1 已把 `--quality` 接到 `quality <-> MSE` 标尺；R2.1 又把 `target_mse` 与 `feedback_loop_trials` 接入 [`src/pipeline.rs`](/Users/5km/Dev/Rust/pngoptim/src/pipeline.rs) 和 [`src/palette_quant.rs`](/Users/5km/Dev/Rust/pngoptim/src/palette_quant.rs)。
- 当前缺口：搜索过程已具备反馈式雏形，但还没有达到 `libimagequant` 那种围绕 `target_mse/max_mse` 的稳定收缩闭环。
- 参考：`quality -> target_mse/max_mse -> feedback loop -> 满足质量约束的更小 palette`。

### 7.2 palette search、remap refine 与 nearest search 已进入正确方向，但仍未达到参考实现

- 当前：R2 已替换掉旧桶统计截断量化器，R2.1 补上了 feedback-style palette search、unused color replacement 和 1 次真实像素 remap 收敛；R2.2 又补上了 VP-tree 风格 nearest search 与 likely-index 剪枝，见 [`src/palette_quant.rs`](/Users/5km/Dev/Rust/pngoptim/src/palette_quant.rs)。
- 当前缺口：仍缺少 `libimagequant` 风格更稳定的 remap-to-palette feedback、dither map 和 selective dithering。

这条链路当前仍然缺少：

1. 更稳定的 feedback loop palette 缩减。
2. remap 阶段继续累计统计并回灌 palette。
3. `dither map + selective Floyd`。

### 7.3 remap / dithering 仍是最大架构缺口

- 当前：[`src/pipeline.rs`](/Users/5km/Dev/Rust/pngoptim/src/pipeline.rs) 已改为优先评估非抖动结果，再在抖动开启时对抖动结果做“择优采用”。
- 当前：[`src/palette_quant.rs`](/Users/5km/Dev/Rust/pngoptim/src/palette_quant.rs) 已补上 VP-tree 风格 nearest，但抖动仍是 naive 全图误差扩散，不是 `libimagequant` 的 dither map + selective Floyd。
- 已验证：naive 全图 Floyd 在真实样本上会同时拉低质量并放大体积，因此只能作为受保护的实验路径，不能视为参考实现等价物。

这意味着当前 `--floyd` 兼容的是参数形状，不是算法行为。

### 7.4 `skip-if-larger` 仍过于粗糙

- 当前：[`src/pipeline.rs`](/Users/5km/Dev/Rust/pngoptim/src/pipeline.rs:115) 仅在“输出大于输入”时失败。
- 参考实现会把质量损失和尺寸收益绑定判断。

## 8. 实测证据：本地样本 spot check

样本：本地 `demo.png`，尺寸 `2706x1776`，输入 `714,783 bytes`。

命令：

```bash
./target/release/pngoptim /Users/5km/Downloads/demo.png --output /tmp/pngoptim-demo-r2.png --force
./target/release/pngoptim /Users/5km/Downloads/demo.png --output /tmp/pngoptim-demo-q6575.png --quality 65-75 --force
pngquant /Users/5km/Downloads/demo.png --output /tmp/pngquant-demo-q6575.png --quality 65-75 --force --verbose
```

结果：

| 场景 | 输出大小 | 质量结果 | 备注 |
|---|---:|---:|---|
| 当前 `pngoptim` 默认输出 | `130,792` bytes | `quality_score=77`, `quality_mse=7.091` | 接入 `importance_map + remap feedback` 后显著改善 |
| 当前 `pngoptim --quality 65-75` | `125,259` bytes | `quality_score=75`, `quality_mse=7.618` | 已满足最低质量门槛，不再因 `actual < minimum` 失败 |
| 参考 `pngquant --quality 65-75` | `136,915` bytes | `MSE=5.210 (Q=82)` | `19` 色，满足质量要求 |

补充观测：

1. R2.1 到 RF-6 的轨道判断是对的：当前主要收益确实来自 palette search + remap 主链，而不是 PNG 编码尾部微调。
2. RF-4 第二段把 `importance_map` 接入 histogram/remap，并让 dither 路径在进入 selective Floyd 前先做一次 plain remap feedback 后，`demo.png` 从长期停滞的 `quality_score=56/57` 直接提升到默认 `77`、`--quality 65-75` 下 `75`。
3. 这说明此前真正缺的不是 nearest search 或候选选择，而是 `importance/remap feedback` 没有贯通到 dither 分支。
4. RF-5 后续又补了透明区域/近透明像素的 plain-match fallback，把参考实现中“静态背景/透明区域避免抖动伪影”的思路收敛成当前 PNG CLI 可落地的版本。
5. RF-6 现已完成：same-score size-aware 决策与 `skip-if-larger` 的 quality/size 联动都已接入，`skip-if-larger` 不再是“只要输出更大就失败”的粗糙规则。
6. RF-7 已全部通过：本地 `quality-size`、`perf`、`stability`、`release-check` 均为 pass，远端 `phase-f-cross-platform` run `22750921042` 也已 success。当前剩余动作主要是评估是否要把显式 background 图像分支产品化。

补充观测（R2.2 / `nearest.rs` 对齐）：

1. `reports/smoke/r2-2-smoke-verify/smoke_report.csv` 显示 perf 样本已明显恢复：
   - `perf-001-large-gradient-noise`: `5085.685 ms`
   - `perf-002-large-alpha-pattern`: `11901.303 ms`
2. 说明 `nearest.rs` 对齐已经解决了此前的大部分搜索性能回退。
3. 在这个前提下，下一步应把主要精力转到 `remap.rs` 的 palette feedback 与 selective dithering，而不是继续在 nearest search 上打补丁。

结论：这不是编码器调优问题，主要差距来自 palette 搜索、remap 与 dithering 算法本身。

## 9. 复刻策略建议（已重规划为 Reference-First 模块路线）

原先的 `R1/R2/R3/R4` 粒度过粗，容易把“参考实现驱动”重新滑回“边写边猜”。当前已改为按参考模块推进：

### RF-1. `pngquant.c` + `attr.rs`

1. 引入 `quality_to_mse()` / `mse_to_quality()`。
2. 让 `--quality` 真正转为 `target_mse` / `max_mse`。
3. 让 `speed` 驱动搜索预算、posterization、dither-map 策略。
4. 状态：`Done`

### RF-2. `quant.rs` + `mediancut.rs` + `kmeans.rs`

1. 引入 gamma-aware 浮点感知空间。
2. 引入感知权重直方图与自适应 posterization。
3. 引入 mediancut + feedback loop。
4. 引入 remap 前后的 k-means refinement。
5. 状态：`Partially Done`
6. 剩余缺口：feedback loop 收缩还不够稳定，remap 后 palette 回灌还没按 `remap_to_palette()` 结构做。

### RF-3. `nearest.rs`

1. 引入 VP-tree nearest。
2. 引入 likely-index 提前命中。
3. 引入 nearest-other-color 距离剪枝。
4. 状态：`Done`
5. 结果：perf 回退已大幅收回。

### RF-4. `remap.rs::remap_to_palette`

1. 对齐 remap 时的 palette 统计回灌。
2. 对齐 background 分支。
3. 对齐 importance-map 权重入口。
4. 对齐 remap error 计算口径。
5. 状态：`Partially Done`
6. 已完成 plain remap 的 palette 统计回灌、importance-map 权重接入，以及 dither 前 remap feedback；剩余显式 background 分支仍待补齐。

### RF-5. `remap.rs::dither_map` + `remap_to_palette_floyd`

1. 引入 dither map。
2. 引入 selective Floyd，而不是全图误差扩散。
3. 对齐 serpentine 扫描与 `max_dither_error`。
4. 对齐 background-aware dithering。
5. 状态：`Partially Done`
6. 已完成 contrast-map 驱动的 selective Floyd core subset，并补上透明区域/近透明像素的 plain-match fallback；剩余显式 background 图像分支仍待补齐。

### RF-6. `pngquant.c` + `quant.rs` 决策层

1. 对齐 `skip-if-larger` 启发式。
2. 对齐 remap 后质量决策与退出条件。
3. 对齐输出决策与质量/尺寸联动逻辑。
4. 状态：`Done`
5. 已完成 same-score 候选的 size-aware 选择，并对齐 `skip-if-larger` 的质量损失/体积收益启发式。

### RF-7. 全门禁收口

1. 重跑 `quality-size`。
2. 重跑 `perf`。
3. 重跑 `stability` / `cross-platform` / `release-check`。
4. 更新公开发布资产与阶段结论。
5. 状态：`Done`
6. 当前结果：本地 `quality-size` / `perf` / `stability` / `release-check` 已全部通过，远端 `phase-f-cross-platform` run `22750921042` 也已 success。

### 最后才回到编码与体积微调

当 RF-4 / RF-5 / RF-6 对齐后，再做：

1. palette 排序策略细化。
2. `tRNS` 裁剪与透明索引排序。
3. filter / deflate 组合回归。
4. `skip-if-larger` 启发式对齐。

## 10. 2026-03-06 静态 PNG 复查结论

用户样本复查表明，之前把静态 PNG 主线直接视为“完全收口”是过早结论。  
从工程链路、CI/CD 和发布资产角度，主线是完整的；但从 `pngquant/libimagequant` 的静态量化算法细节看，仍存在一批会直接影响平滑阴影、渐变和 `--quality` 用户体验的 reference drift。

### 10.1 样本实测

样本：`/Users/5km/Downloads/demo.png`

参考命令：

```bash
./target/release/pngoptim /Users/5km/Downloads/demo.png --output /tmp/pgo-noq-audit.png --force
./target/release/pngoptim /Users/5km/Downloads/demo.png --output /tmp/pgo-q6575-audit.png --quality 65-75 --force
pngquant /Users/5km/Downloads/demo.png --output /tmp/pngq-q6575-audit.png --quality 65-75 --force --verbose
```

本轮确认的结果：

| 场景 | 输出大小 | 质量结果 | 耗时 | 说明 |
|---|---:|---:|---:|---|
| 当前 `pngoptim --quality 65-75` | `144,803` bytes | `quality_score=89`, `quality_mse=3.105` | `0.73s` | 默认抖动继续向参考收敛 |
| 当前 `pngoptim --quality 65-75 --floyd=0.5` | `133,985` bytes | `quality_score=90`, `quality_mse=2.789` | `参考样本 spot check` | 半强度抖动已非常接近参考体积 |
| 当前 `pngoptim --quality 65-75 --nofs` | `107,965` bytes | `quality_score=91`, `quality_mse=2.430` | `0.72s` | 原始带 ICC 输入上已接近 `pngquant --nofs` |
| `pngquant --quality 65-75` | `136,915` bytes | `MSE=5.210 (Q=82)` | `0.55s` | 参考实现 |
| `pngquant --quality 65-75 --nofs` | `104,038` bytes | `参考样本` | `0.45s` | 对照无抖动路径 |

补充观测：

1. `--quality 65-75` 路径已经不再依赖“baseline + targeted”双候选，慢路径缩短到了约 `1s`。
2. 当前真正的硬根因已经确认有两处：
   - 之前引入的 ICC 像素转换会把这张图的唯一颜色数从 `1499` 膨胀到 `9347`
   - remap / Floyd 前没有像 `init_int_palette()` 那样先按输出精度 round palette
3. 去掉坏的 ICC 转换、补齐“先 round 再 remap/dither”后，再把大图无 dither-map 时的 `edges` fallback 接回主线，原始带 ICC 输入上的默认抖动和半强度抖动都继续缩小，`--nofs` 保持在接近 `pngquant --nofs` 的区间。
4. 这说明静态 PNG 主链当前剩余差距已从“基础量化完全跑偏”收敛到少量 palette 落点和大图 Floyd 细节，而不是继续在 histogram / mediancut 外层补护栏。

### 10.2 本轮已修复的偏差

1. 去掉 [`src/pipeline.rs`](/Users/5km/Dev/Rust/pngoptim/src/pipeline.rs) 中外层 `quality -> colors` 二分搜索。
2. 将 `kmeans_iteration_limit` 接入 [`src/quality.rs`](/Users/5km/Dev/Rust/pngoptim/src/quality.rs) 和 [`src/palette_quant.rs`](/Users/5km/Dev/Rust/pngoptim/src/palette_quant.rs)，预算口径向 `attr.rs` 靠拢。
3. 将 feedback loop 的 K-Means 试探改回“每轮 1 次主迭代 + 最终单独 refine”的结构，不再在 trial 阶段连续做多轮收敛。
4. 去掉 [`src/palette_quant.rs`](/Users/5km/Dev/Rust/pngoptim/src/palette_quant.rs) 中自拟的 `refine_palette_from_pixels()`。
5. 将 `find_best_palette()` 的 trial 失败惩罚、`kmeans adjust_weight` 公式和 unused color replacement 进一步对齐到 `libimagequant/src/quant.rs` / `kmeans.rs`。
6. 让 plain remap 的 `palette_error` 真正进入 dithering 的误差阈值决策。
7. 去掉 `--quality` 模式额外跑 baseline 候选的自定义比较，质量请求现在只走目标约束候选。
8. 去掉 `src/pipeline.rs` 中 plain/dither 候选赛马，改回“用户开了抖动就走抖动”的参考语义。
9. 去掉 `src/palette_quant.rs` 中 remap 前按 8-bit RGBA 提前去重的错误收缩，保留内部 float palette 到 remap 完成后再自然收缩。
10. 补齐 `--floyd` 的 `0..1` 强度参数，允许 `--floyd=0.5` 这类中间档，默认 `--floyd` 仍为 `1`。
11. 对齐了 `hist.rs + mediancut.rs` 的两类关键细节：
   - histogram 不再做桶内平均色，改为“代表色 + perceptual weight + 16 cluster 起始箱”
   - mediancut 改为带 `total_box_error_below_target()` / `max_mse_per_color` / best-box split 的误差约束切分
12. 移除了当前有害的 ICC 像素转换支路：现在保留 decoder 输出像素与原始色彩元数据，不再用错误转换把颜色基数从 `1499` 扩张到 `9347`。
13. 将 indexed PNG 编码默认策略对齐到 `pngquant` 的 `PNG_FILTER_NONE + Deflate Level(9)`，并在 `speed >= 10` 时降到 `Level(1)`。
14. 将 plain remap / Floyd 的 palette 使用顺序改回与 `init_int_palette()` 一致：先按输出 posterize 精度 round palette，再做 remap/dither。
15. 对大图默认 Floyd 补上一次 plain remap feedback，让未生成 dither-map 的路径也能拿到更接近 `remap_to_palette()` 的 full-image finalize。
16. 保留 contrast maps 的 `edges` 图，并在大图未生成 dither-map 时退回使用 `edges` 作为选择性抖动图，而不是直接做“裸 Floyd”。

### 10.3 仍然存在的关键偏差

1. 当前 plain remap / dither remap 还没有完整实现 `remap.rs::remap_to_palette()` 的 full-image K-Means finalize 结构。
2. 当前 selective dithering 虽然已接入 core subset，但还没达到 `remap_to_palette_floyd()` 的整套 chunk warmup / background-aware / guess 策略。
3. 默认抖动路径当前仍比 `pngquant` 默认路径大约高 `7.9KB`，说明大图 Floyd 主链已基本回到正确方向，但 palette 落点与剩余 diffusion 细节还要继续收口。
4. `--quality` 路径速度已明显改善，但相对 `pngquant` 仍慢约 `2x`，说明 quantizer 内部仍有可继续收紧的预算与 finalize 开销。

## 11. 当前判断

1. 静态 PNG 主线不该再被笼统写成“彻底完成”；更准确的状态是“工程主线完成，但静态量化算法仍在 reference-first 复查和收口中”。
2. 这不是要推翻现有 Rust 主线，而是要把剩余偏差重新压回参考实现模块上去解决。
3. 继续推进的优先级应该是：
   1. `remap.rs::remap_to_palette` 的 full-image finalize 对齐
   2. `remap.rs::remap_to_palette_floyd` 的剩余视觉细节对齐
   3. 默认无 `--quality` 路径的颜色收缩行为与编码联动校回
   4. 最后再继续压 `--quality` 模式的速度
