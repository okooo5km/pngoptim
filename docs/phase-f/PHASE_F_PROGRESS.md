# Phase F Progress

> 阶段：F（稳定性与跨平台收口）  
> 更新日期：2026-03-06

## F1. 鲁棒性

- [x] 异常输入处理：robustness 样本回归已覆盖  
  证据：`reports/stability/stability-v1-20260305-f2/stability_report.csv`
- [x] 回归零崩溃：`crash_like_count=0`
- [x] fuzz 零崩溃：`panic_count=0`、`signal_count=0`、`timeout_count=0`
- [x] 证据：`reports/stability/stability-v1-20260305-f2/fuzz_summary.json`

## F2. 跨平台

- [x] 本地 collect/aggregate（partial）链路可跑  
  证据：`reports/cross_platform/cross-platform-v1-20260305-f2/summary.md`
- [x] 三平台 CI matrix 流程已落地  
  证据：`.github/workflows/phase-f-cross-platform.yml`
- [x] Phase-F 跨平台门禁改为 Rust 原生 `xtask` 编排（去除 Python 运行时依赖）  
  证据：`src/bin/xtask.rs`
- [x] 三平台一致性最终结论（CI 收口通过）  
  证据：`docs/phase-f/CROSS_PLATFORM_REPORT_V1.md`

## F3. 发布门禁

- [x] release 构建链路已用于阶段 F 核心脚本
- [x] 阶段 F 报告模板与结构化产物已落地
- [x] RC Candidate（阶段出口已达成）  
  证据：`docs/phase-f/RC_CANDIDATE_V1.md`

## 阶段证据

1. 稳定性报告：`docs/phase-f/STABILITY_REPORT_V1.md`
2. 跨平台报告：`docs/phase-f/CROSS_PLATFORM_REPORT_V1.md`
3. 稳定性主产物：`reports/stability/stability-v1-20260305-f2/summary.md`
4. 跨平台本地产物：`reports/cross_platform/cross-platform-v1-20260305-f2/summary.md`
5. 跨平台门禁工作流：`.github/workflows/phase-f-cross-platform.yml`
6. RC 结论：`docs/phase-f/RC_CANDIDATE_V1.md`

## 阶段结论

- [x] 阶段 F 当前状态：`Done`
