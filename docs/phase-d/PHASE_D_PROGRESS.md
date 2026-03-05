# Phase D Progress

> 阶段：D（质量与体积复刻）  
> 更新日期：2026-03-05

## D1. 质量对齐

- [x] 主指标达标（SSIM）：`reports/quality-size/quality-size-v1-20260305-r3/quality_report.csv`
- [x] 辅指标达标（PSNR）：`reports/quality-size/quality-size-v1-20260305-r3/quality_report.csv`

## D2. 体积对齐

- [x] 平均体积差达标：`-0.470220`
- [x] 中位数体积差达标：`-0.435157`
- [x] P95 体积差达标：`-0.123669`
- [x] 证据：`reports/quality-size/quality-size-v1-20260305-r3/size_report.csv`

## D3. 专项修复

- [x] 透明边缘样本：`quality-002-ui-alpha-icon` 已从正回归修复为负回归
- [x] 低色样本：`quality-003-lowcolor-blocks` 保持领先
- [x] UI/渐变样本：`quality-001-gradient-photo` 与 `perf-001-large-gradient-noise` 已收口

## D4. 失败样本闭环

- [x] 建立 top 退化样本清单：`reports/quality-size/quality-size-v1-20260305-r3/top_regressions.json`
- [x] 本轮未出现正体积回归样本（P95 < 0）

## 阶段证据

1. 阶段报告：`docs/phase-d/QUALITY_SIZE_REPORT_V1.md`
2. 质量/体积评测命令：`cargo run --release --bin xtask -- quality-size --run-id <run_id> --candidate target/release/pngoptim --build`
3. 主评测结果：`reports/quality-size/quality-size-v1-20260305-r3/summary.md`
4. 回归稳定性：`reports/smoke/smoke-v1-20260305-d-encoding/summary.md`
5. 兼容性回归：`reports/compat/compat-v1-20260305-d-encoding/summary.md`

## 阶段结论

- [x] 阶段 D 出口条件已满足：`Quality & Size Report v1`
