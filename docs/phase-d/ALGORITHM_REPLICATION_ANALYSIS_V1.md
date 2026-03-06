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

## 7. 当前项目与参考实现的关键差距

以下差距不是调参问题，而是架构级差距：

### 7.1 质量语义偏差

- 当前：[`src/cli.rs`](/Users/5km/Dev/Rust/pngoptim/src/cli.rs:12) 将 `quality` 取中值。
- 当前：[`src/pipeline.rs`](/Users/5km/Dev/Rust/pngoptim/src/pipeline.rs:82) 用中值直接驱动色数。
- 参考：`quality -> target_mse/max_mse -> palette search`。

### 7.2 量化器过于简化

- 当前：[`src/palette_quant.rs`](/Users/5km/Dev/Rust/pngoptim/src/palette_quant.rs:29) 只做 5/5/5/4 桶统计。
- 当前：[`src/palette_quant.rs`](/Users/5km/Dev/Rust/pngoptim/src/palette_quant.rs:40) 直接按频率排序截断。
- 当前：[`src/palette_quant.rs`](/Users/5km/Dev/Rust/pngoptim/src/palette_quant.rs:119) remap 时仍是线性最近邻。

这条链路没有：

1. 感知权重直方图。
2. mediancut 搜索。
3. feedback loop。
4. k-means refinement。
5. VP-tree nearest。

### 7.3 质量指标错误

- 当前：[`src/pipeline.rs`](/Users/5km/Dev/Rust/pngoptim/src/pipeline.rs:94) 使用 `mean_abs_diff` 评分。
- 当前：[`src/pipeline.rs`](/Users/5km/Dev/Rust/pngoptim/src/pipeline.rs:399) 将其线性映射回 `0..100`。

参考实现使用的是 MSE 驱动的质量标尺，并把质量门限直接参与 palette 搜索。

### 7.4 抖动逻辑未真正接入主线

- 当前：[`src/pipeline.rs`](/Users/5km/Dev/Rust/pngoptim/src/pipeline.rs:84) `dither` 只是把 `max_colors` 减 8。
- 当前主线没有 dither map、selective Floyd、background-aware remap。

这意味着当前 `--floyd` 兼容的是参数形状，不是算法行为。

### 7.5 `skip-if-larger` 过于粗糙

- 当前：[`src/pipeline.rs`](/Users/5km/Dev/Rust/pngoptim/src/pipeline.rs:115) 仅在“输出大于输入”时失败。
- 参考实现会把质量损失和尺寸收益绑定判断。

## 8. 实测证据：本地样本 spot check

样本：本地 `demo.png`，尺寸 `2706x1776`，输入 `714,783 bytes`。

命令：

```bash
./target/release/pngoptim /Users/5km/Downloads/demo.png --output /private/tmp/pngoptim-demo-out.png --quality 65-75 --force
pngquant /Users/5km/Downloads/demo.png --output /private/tmp/pngquant-demo-out.png --quality 65-75 --force --verbose
```

结果：

| 工具 | 输出大小 | palette 实际用色数 | 备注 |
|---|---:|---:|---|
| 当前 `pngoptim` | `150,914` bytes | `92` | CLI 输出 `requested_quality=65-75, quality_score=99` |
| 参考 `pngquant` | `136,915` bytes | `19` | verbose 显示 `MSE=5.210 (Q=82)` |

补充观测：

1. 当前实现即使用了更多颜色，结果仍然更大。
2. 当前实现的平均绝对误差约为 `3.5427`。
3. 同一标尺下，`pngquant` 的平均绝对误差约为 `0.3521`。
4. 因此它不是“更小但更糊”，而是“更小且更准”。

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
2. 但在“算法级复刻 pngquant/libimagequant”这个新目标下，当前核心量化实现不能视为完成。
3. 下一步不应继续微调当前桶统计量化器，而应按参考实现主链逐段重建。
4. 复刻优先级应回到 Phase D 核心：先对齐质量语义、palette 搜索、remap/dither，再重新跑 E/F/G 回归。
