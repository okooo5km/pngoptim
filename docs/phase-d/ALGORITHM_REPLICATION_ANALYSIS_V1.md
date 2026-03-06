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
| 当前 `pngoptim` 默认输出 | `131,278` bytes | `quality_score=56`, `quality_mse=14.644` | R2.1 后明显改善，但仍未达到参考质量 |
| 当前 `pngoptim --quality 65-75` | 无输出 | `actual=57, minimum=65` | 按新门禁正确失败，说明仍有质量缺口 |
| 参考 `pngquant --quality 65-75` | `136,915` bytes | `MSE=5.210 (Q=82)` | `19` 色，满足质量要求 |

补充观测：

1. R2.1 证明当前主要收益确实来自 palette search + remap refine，而不是 PNG 编码尾部微调。
2. 在真正可比的 `--quality 65-75` 场景下，当前实现仍直接失败，而 `pngquant` 可以在满足质量门槛的同时输出 `136,915` bytes。
3. 这说明当前缺口已经从“完全走错方向”收窄为“还差更成熟的 remap/search/dither 主链”。

补充观测（R2.2 / `nearest.rs` 对齐）：

1. `reports/smoke/r2-2-smoke-verify/smoke_report.csv` 显示 perf 样本已明显恢复：
   - `perf-001-large-gradient-noise`: `5085.685 ms`
   - `perf-002-large-alpha-pattern`: `11901.303 ms`
2. 说明 `nearest.rs` 对齐已经解决了此前的大部分搜索性能回退。
3. 在这个前提下，下一步应把主要精力转到 `remap.rs` 的 palette feedback 与 selective dithering，而不是继续在 nearest search 上打补丁。

结论：这不是编码器调优问题，主要差距来自 palette 搜索、remap 与 dithering 算法本身。

## 9. 复刻策略建议

建议按下面顺序替换核心实现，而不是继续在当前量化器上打补丁：

### R1. 先复刻 `quality` / `speed` 语义

1. 引入 `quality_to_mse()` / `mse_to_quality()`。
2. 让 `--quality` 真正转为 `target_mse` / `max_mse`。
3. 让 `speed` 驱动搜索预算、posterization、dither-map 策略。

### R2. 重写 histogram 与 palette search

1. 引入 gamma-aware 浮点感知空间。
2. 引入感知权重直方图与自适应 posterization。
3. 引入 mediancut + feedback loop。
4. 引入 remap 前后的 k-means refinement。

### R3. 重写 remap / dither 路径

1. 引入 VP-tree nearest。
2. 引入 remap 阶段 palette 再收敛。
3. 引入 selective Floyd + dither map。
4. 再对 `--floyd`、`--ordered`、背景透明处理做兼容性校验。

### R4. 最后回到编码与体积微调

当 palette / remap 算法对齐后，再做：

1. palette 排序策略细化。
2. `tRNS` 裁剪与透明索引排序。
3. filter / deflate 组合回归。
4. `skip-if-larger` 启发式对齐。

## 10. 本轮结论

1. 当前项目已经完成 Rust 工程化与发布链路，不再依赖 Python 编排。
2. R1 已完成质量标尺纠偏，R2.1 已把量化器推进到 feedback-style search + 像素级 remap refine，R2.2 又补上了 VP-tree 风格 nearest；离参考实现的主要剩余差距已经集中到 `remap.rs`。
3. 下一步不应回退到“再打一层小补丁”，而应继续完成 remap-to-palette feedback、dither map 和 selective dithering。
4. 复刻优先级应回到 Phase D 核心：先对齐质量语义、palette 搜索、remap/dither，再重新跑 E/F/G 回归。
