# APNG Optimization Plan v1

> 阶段：H（APNG 动图压缩优化）  
> 更新日期：2026-03-06

## 0. 当前执行门槛

Phase H 规划与 H1 骨架已经建立，但当前执行状态是 `Blocked`。

原因：

1. 静态 PNG `reference-first` 复查仍未收口。
2. `demo.png` 这类平滑阴影/UI 样本的默认抖动路径仍有可感知阶梯感。
3. 在静态主线还存在这类质量偏差时，不适合继续推进 APNG 产品化，否则会把未收口的 palette/remap/dither 主链放大到动画场景。

恢复 Phase H 的前提：

1. 静态 PNG 当前主阻塞清理完成。
2. `demo.png --quality 65-75` 默认抖动路径达到可接受视觉质量。
3. 再从 H1 的 CLI / pipeline 接入开始恢复，而不是直接跳到 H2/H3。

## 1. 目标

在现有静态 PNG 主线已经完成复刻和发布收口的前提下，新增 APNG 动图优化能力，并形成相对当前对标工具更强的产品能力：

1. 正确解析、合成、重写 APNG。
2. 先提供稳定的 lossless APNG 结构优化。
3. 再把现有静态 PNG 的量化主链升级为 animation-aware 的 APNG 有损优化器。
4. 全过程继续保持 Rust-only 主线、跨平台、可评测、可回归。

## 2. 一手规范结论（冻结实现约束）

### 2.1 APNG 现在是正式 PNG 规范的一部分

根据 `PNG 3`，PNG 已明确覆盖静态与动画栅格图像，并定义了 `image/apng` 媒体类型。APNG 不再只是历史扩展实现，而是当前正式规范的一部分。

这意味着：

1. 阶段 H 可以直接以 `PNG 3` 作为主规范，不需要围绕“非正式扩展”做兼容赌博。
2. 工程设计应优先满足标准语义，而不是只对某一浏览器实现做适配。

### 2.2 APNG 的核心 chunk 模型

APNG 在普通 PNG datastream 上增加三类关键 chunk：

1. `acTL`：动画级控制，定义总帧数与循环次数。
2. `fcTL`：帧级控制，定义帧矩形、偏移、延迟、`dispose_op`、`blend_op`。
3. `fdAT`：除默认图像以外的帧数据块，本质上是带 sequence number 的 `IDAT` 数据承载。

实现含义：

1. `acTL` 必须在首个 `IDAT` 之前出现，文件才被识别为 APNG。
2. 第一帧既可能是默认图像，也可能只是动画前的 thumbnail；这取决于首个 `IDAT` 前是否存在对应的 `fcTL`。
3. 所有 `fcTL` / `fdAT` 共享单调递增的 sequence number，错误排序必须视为格式错误。

### 2.3 APNG 不是“每帧各自一个 PNG 文件”

这是阶段 H 最重要的架构约束。

APNG 所有动画帧共享一套全局 PNG 头部语义：

1. 全局 `IHDR` 决定色彩类型、位深、压缩/过滤/隔行模式。
2. 若使用 indexed color，则全动画共享同一套 `PLTE/tRNS`。
3. 帧级 `fcTL` 只允许改变子矩形尺寸与位置，不允许为每一帧单独定义新的调色板或新的 PNG 色彩类型。

直接结论：

1. 不能把 APNG 设计成“逐帧跑一次当前静态 pipeline，然后再简单拼接”。
2. 如果要做 indexed APNG，有损优化必须基于整个动画做全局颜色决策。
3. 帧级优化的真正自由度在于：
   - 子矩形裁剪
   - `dispose_op` / `blend_op` 选择
   - 帧时序折叠
   - 帧数据重编码

### 2.4 Canvas / output buffer 语义是正确性的核心

APNG 每一帧的视觉结果不是独立图片，而是以下语义的结果：

1. 先在同一逻辑 canvas 上应用上一帧的 dispose。
2. 再按当前帧的 `blend_op` 把 subframe 混合到 canvas。
3. 然后把 composited canvas 作为观测输出。

因此实现必须区分至少三种层级：

1. 原始 subframe 数据
2. composited canvas 数据
3. dispose 前 / dispose 后的状态

如果没有这一层，后续的裁剪、去重、delta 优化都会错。

## 3. 对当前仓库的架构映射

## 3.1 当前静态主线

当前静态处理链在 [pipeline.rs](/Users/5km/Dev/Rust/pngoptim/src/pipeline.rs)：

1. `image::load_from_memory_with_format(..., ImageFormat::Png)` 直接把输入当作单张 PNG 解码。
2. 量化主链已经完成 `pngquant/libimagequant` 风格复刻。
3. 写出走 `png::Encoder` 的 indexed PNG 路径。

这条链对静态 PNG 已经足够，但对 APNG 还缺三层能力：

1. animation parser / frame model
2. canvas composer
3. frame-aware writer / optimizer

## 3.2 现有依赖已经提供的基础

本仓库当前依赖不需要推翻：

1. `png 0.18.1`
   - 已有 `AnimationControl`
   - 已有 `FrameControl`
   - 已有 `Reader::next_frame`
   - 已有 `Encoder::set_animated`
2. `image 0.25.9`
   - 已有 `ApngDecoder`
   - 内部已经实现 dispose / blend / compositing 参考逻辑

规划结论：

1. APNG 支持不需要切换语言或引入外部脚本。
2. 推荐继续以 `png` crate 为底层读写主干。
3. `image` crate 的 `ApngDecoder` 更适合作为语义参考，而不是最终优化核心。

原因很直接：

1. 我们需要保留并重写 APNG frame metadata。
2. 我们需要控制 subframe、canvas、矩形、sequence number 与写回顺序。
3. 这些需求用 `png` 的低层 reader / writer 更贴近目标。

## 4. 新的实现路线

阶段 H 不走“一上来就做全动画有损量化”的路线，而是分成两段：

1. 先把 APNG 当作结构化容器，做语义正确的 lossless 优化。
2. 再把静态量化主链升级为 animation-aware 全局量化器。

这样做的原因：

1. 先建立 parser / canvas / writer 与测试基线，风险更可控。
2. 有损优化的正确性依赖前面的 compositing 模型；先把基础做对，后面量化才能稳定。
3. 这也能尽快产出一条“对现有工具有增量价值”的能力线，因为连 `oxipng` 官方都明确写了它对 APNG 目前只有有限优化支持。

## 5. 阶段 H 细分规划

### H1. APNG 格式与语义基线

目标：建立完整、可 round-trip 的 APNG 内部模型。

交付：

1. `ApngSource` / `ApngFrame` / `ApngCanvasFrame` 数据结构
2. `acTL` / `fcTL` / `fdAT` 解析
3. default image 是否计入动画的明确建模
4. dispose / blend compositing 语义测试
5. metadata preserve / strip 在 APNG 下的行为约束

实现建议：

1. 新建 `src/apng/` 模块，而不是把动画逻辑塞进现有静态 `pipeline.rs`
2. 读入阶段以 `png::Decoder` + `Reader::next_frame` 为主
3. 语义对照 `image` crate 的 APNG compositing 逻辑建立单测

阶段出口：

1. 能正确读取 APNG 并输出 composited frames
2. 能在不改变视觉结果的前提下 round-trip 写回 APNG

### H2. Lossless APNG 结构优化

目标：在不改变每一帧视觉输出的前提下，先做结构层压缩。

优先优化点：

1. 重复帧折叠
   - 完全相同的 composited frame 直接合并 delay
2. 子矩形裁剪
   - 从 composited delta 中找最小变化矩形
3. `blend_op` / `dispose_op` 重新选择
   - 在等价显示结果下选更小编码矩形或更可压缩的模式
4. 帧数据重过滤与重压缩
5. ancillary metadata 策略复用静态主线

阶段出口：

1. 建立 `apng-lossless` 模式
2. 在 APNG 样本集上得到稳定的负体积回归
3. 视觉逐帧比对零差异

### H3. Animation-Aware 全局量化

目标：把现有静态量化能力扩展到动画整体，而不是逐帧分裂处理。

核心策略：

1. 建立 animation-wide histogram
2. 对帧和像素做时间权重 / 可见性权重
3. 搜索全局 palette，而不是每帧独立 palette
4. 将现有 `palette search + remap + selective dithering` 用在 animation-wide 颜色决策上

注意：

1. 因为 APNG 共享全局色彩类型与调色板，若选择 indexed APNG，就必须接受单一全局 palette 约束。
2. 因此需要把“全局 indexed APNG”与“真彩 APNG + 结构优化”都作为候选路径。
3. 阶段 H 的有损优化不应默认假设 indexed 一定更优。

阶段出口：

1. `--quality` / `--speed` / `--floyd` 在 APNG 上有明确语义
2. 建立动画级质量评分与 whole-file `skip-if-larger`

### H4. APNG 专项优化

目标：做静态 pngquant 不覆盖的动画专项策略。

专项方向：

1. 帧重要性建模
   - 按显示时长、出现面积、视觉变化量加权
2. 局部高质量保留
   - 高频边缘 / 半透明边缘 / UI 元素给更高权重
3. 动画级 dither 策略
   - 减少闪烁而不是只最小化单帧误差
4. 帧矩形与量化联动
   - 先裁剪再量化与先量化再裁剪都要做 A/B

阶段出口：

1. 在 APNG 数据集上形成相对 lossless H2 的稳定进一步收益
2. 不引入明显 temporal flicker 退化

### H5. 评测与 CI

目标：让 APNG 进入主线门禁，而不是停留在实验功能。

新增门禁：

1. APNG compatibility
2. APNG quality-size
3. APNG perf
4. APNG cross-platform consistency

建议新增数据集类型：

1. 透明边缘 icon/贴纸类
2. UI 动效类
3. 渐变与照片混合类
4. 大面积静止 + 小区域变化类
5. 重复帧 / 高频闪烁类

### H6. 产品化与 CLI

目标：让 APNG 成为可发布能力，而不是隐藏实验开关。

CLI 方向：

1. 自动识别 APNG 输入并走动画 pipeline
2. 对静态 PNG 维持现有行为不变
3. 逐步补齐 APNG 专属选项，但初期优先少而稳

初期建议避免一次性暴露过多参数。

更稳的做法：

1. 默认支持 APNG 输入
2. 先提供 `--apng-mode lossless|lossy|auto`
3. 其余沿用现有 `--quality` / `--speed` / `--strip` / `--skip-if-larger`

## 6. 验收门禁

### H1 门槛

1. APNG decode / compose / round-trip 单测全绿
2. default image / hidden first frame / sequence number / dispose / blend 覆盖

### H2 门槛

1. 视觉零差异
2. lossless APNG 样本集均值体积下降
3. 跨平台字节级或统计一致性达标

### H3-H4 门槛

1. whole-animation 质量指标达标
2. temporal artifact 检查通过
3. whole-file 体积统计优于 H2 lossless 基线

### H5-H6 门槛

1. 进入 `xtask` 主线
2. GitHub Actions 三平台回归通过
3. 用户文档与复现文档补齐

## 7. 当前结论

1. APNG 是一个合理且有价值的“超越阶段”优先项。
2. 这条路线与我们当前 Rust-only、reference-first、许可证安全的约束不冲突。
3. 技术上不需要推翻现有依赖栈，但必须新增独立的 animation pipeline。
4. 首个编码目标应是 `H1 + H2`，不要直接跳到全动画有损量化。

## 8. 参考资料

1. `PNG 3` 正式规范：<https://www.w3.org/TR/png-3/>
2. Mozilla APNG 规范历史说明：<https://wiki.mozilla.org/APNG_Specification>
3. `png` crate 源码（本地依赖，APNG 读写结构）：
   - `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/png-0.18.1/src/common.rs`
   - `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/png-0.18.1/src/decoder/mod.rs`
   - `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/png-0.18.1/src/encoder.rs`
4. `image` crate APNG 合成参考（本地依赖）：
   - `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/image-0.25.9/src/codecs/png.rs`
5. `oxipng` APNG 支持现状（作为竞品边界参考）：<https://github.com/oxipng/oxipng>
