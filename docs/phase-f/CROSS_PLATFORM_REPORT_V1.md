# Cross-platform Report v1

> 更新日期：2026-03-05  
> 阶段：F（稳定性与跨平台收口）  
> 编排命令：`cargo run --release --bin xtask -- cross-platform <collect|aggregate>`

## 1. 结论摘要（当前本地证据）

本地已完成 collect + aggregate（partial）验证：
1. run_id：`cross-platform-v1-20260305-f2`
2. 平台：`macos-local-arm64`
3. collect 结果：`sample_count=7`、`success_count=7`
4. 守护结果：`smoke/compat/stability` 全通过

partial 聚合证据：
1. `reports/cross_platform/cross-platform-v1-20260305-f2/summary.md`
2. `reports/cross_platform/cross-platform-v1-20260305-f2/consistency.csv`

## 2. 三平台收口机制

已新增 CI workflow：
1. `.github/workflows/phase-f-cross-platform.yml`

流程：
1. `collect`：`ubuntu-latest` / `macos-latest` / `windows-latest` 三平台并行执行（Rust `xtask`）。
2. `aggregate`：汇总三平台 JSON，生成一致性报告并门禁失败（Rust `xtask`）。

## 3. 一致性判定项

1. 平台数门禁（默认要求 `>=3`，否则失败）。
2. 体积统计一致性：
   - `size_ratio_mean`
   - `size_ratio_median`
   - `size_ratio_p95`
3. 行为守护一致性：
   - smoke 全通过
   - compatibility 全通过
   - stability（crash_like/failures）为 0
4. 样本级输出一致性：
   - `output_bytes` 跨平台一致

## 4. 产物清单

1. `reports/cross_platform/<run_id>/platform/*.json`
2. `reports/cross_platform/<run_id>/consistency.csv`
3. `reports/cross_platform/<run_id>/inconsistent_samples.json`
4. `reports/cross_platform/<run_id>/summary.md`

## 5. 当前状态

1. 本地验证：完成（单平台 partial）。
2. 三平台最终收口：待 CI matrix 完整跑数后确认（DOD-11）。
