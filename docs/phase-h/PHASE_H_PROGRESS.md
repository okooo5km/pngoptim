# Phase H Progress

> 阶段：H（APNG 动图压缩优化）  
> 更新日期：2026-03-06

## H1. 格式与语义基线

- [x] 完成 APNG 一手规范调研
- [x] 完成当前仓库与依赖能力映射
- [x] 冻结 Phase H 技术路线：`docs/phase-h/APNG_OPTIMIZATION_PLAN_V1.md`
- [ ] 建立 `src/apng/` 模块骨架
- [ ] 建立 APNG decode / compose / round-trip 单测

## H2. Lossless 结构优化

- [ ] 重复帧折叠
- [ ] 帧矩形最小化
- [ ] `dispose_op` / `blend_op` 等价重写
- [ ] frame-level refilter / recompress

## H3. Animation-Aware 有损优化

- [ ] animation-wide histogram
- [ ] 全局 palette search
- [ ] animation-aware remap / selective dithering
- [ ] whole-file `skip-if-larger`

## H4. 评测与门禁

- [ ] APNG 数据集清单
- [ ] `xtask apng-compat`
- [ ] `xtask apng-quality-size`
- [ ] `xtask apng-perf`
- [ ] 三平台回归

## 阶段证据

1. 规划文档：`docs/phase-h/APNG_OPTIMIZATION_PLAN_V1.md`
2. 阶段记忆：`AGENTS.md`

## 阶段结论

- [x] 阶段 H 当前状态：`In Progress`
- [x] 当前优先级：先做 `H1` 与 `H2`，暂不直接跳到全动画有损量化
