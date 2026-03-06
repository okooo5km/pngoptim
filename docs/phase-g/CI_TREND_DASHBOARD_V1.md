# CI Trend Dashboard v1

> 更新日期：2026-03-06  
> 阶段：G（开源发布与社区协作）

## 1. 目标

建立长期可复用的 CI 趋势看板，用统一方式观察：
1. `nightly-regression`
2. `phase-f-cross-platform`

## 2. 生成方式

本地或 CI 均使用 Rust 原生命令：

```bash
cargo run --release --bin xtask -- ci-trends --run-id ci-trends-v1 --repo okooo5km/pngoptim --lookback 20
```

定时工作流：
1. `.github/workflows/ci-trend-dashboard.yml`
2. 触发方式：`schedule` + `workflow_dispatch`

## 3. 输出产物

1. `reports/trends/<run_id>/summary.md`
2. `reports/trends/<run_id>/workflow_runs.json`
3. `reports/trends/<run_id>/workflow_summary.csv`

## 4. 当前样本结论

最新本地验证 run_id：`ci-trends-v1-20260306`

证据：
1. `reports/trends/ci-trends-v1-20260306/summary.md`
2. `reports/trends/ci-trends-v1-20260306/workflow_summary.csv`

观察：
1. `phase-f-cross-platform` 最近 10 次运行中 `7` 次成功、`3` 次失败，成功率 `70.0%`。
2. 最新一次 `phase-f-cross-platform` 为成功：`22728357096`。
3. `nightly-regression` 当前尚无历史 run，dashboard 会明确标记为 `no runs found`，但机制已可用。

## 5. 用途

1. 观察失败到修复的收敛过程。
2. 识别高频失败 workflow。
3. 为阶段 G 持续演进条目提供结构化证据。
