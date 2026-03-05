# Perf Report v1

> 更新日期：2026-03-05  
> 阶段：E（性能优化冲刺）  
> 主运行证据：`reports/perf/perf-v1-20260305-e5/`

## 1. 结论摘要

1. 样本总数：`7`，每样本迭代：`2`
2. 失败数：`0`
3. 平均耗时（ms）：
   - baseline：`248.465`
   - candidate：`155.674`
   - 速度比（baseline/candidate）：`1.596`
4. 样本级 P95 速度比（baseline/candidate）：`4.487`
5. 峰值 RSS（KB）：
   - baseline：`202768384.0`
   - candidate：`60719104.0`

判定：满足“至少一项性能指标优于基线”（DOD-08），并完成资源画像（DOD-09）。

## 2. 本轮优化与可观测性

代码位置：
1. `src/pipeline.rs`
2. `src/palette_quant.rs`
3. `src/bin/xtask.rs`（`perf` 子命令）

主要动作：
1. 新增模块级耗时埋点（decode/quantize/encode/total），通过环境变量 `PNGOPTIM_PROFILE_METRICS=1` 输出。
2. 编码阶段改为 speed 分档策略（压缩等级与过滤器候选集绑定）。
3. 量化路径改为固定桶数组索引，降低 HashMap 热点成本。
4. `xtask perf` 默认使用 `release` 二进制，输出 `perf_compare.csv` 与 `memory_profile.json`。

## 3. 模块级耗时画像（candidate 平均，ms）

1. decode：`13.565`
2. quantize：`7.008`
3. encode：`112.778`
4. total：`133.351`

观察：当前主要热点仍在 `encode`，后续可在阶段 F/G 前继续做编码路径专项优化。

## 4. 产物清单

1. `reports/perf/perf-v1-20260305-e5/summary.md`
2. `reports/perf/perf-v1-20260305-e5/perf_compare.csv`
3. `reports/perf/perf-v1-20260305-e5/perf_aggregate.csv`
4. `reports/perf/perf-v1-20260305-e5/memory_profile.json`
5. `reports/perf/perf-v1-20260305-e5/failures.json`

## 5. 守护回归

1. smoke：`reports/smoke/smoke-v1-20260305-e/summary.md`（9/9）
2. compatibility：`reports/compat/compat-v1-20260305-e/summary.md`（全通过）
3. 质量体积守护：`reports/quality-size/quality-size-v1-20260305-e-guard-r3/summary.md`（无失败）
