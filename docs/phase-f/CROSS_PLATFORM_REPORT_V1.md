# Cross-platform Report v1

> 更新日期：2026-03-06  
> 阶段：F（稳定性与跨平台收口）  
> 编排命令：`cargo run --release --bin xtask -- cross-platform <collect|aggregate>`

## 1. 结论摘要（CI 三平台证据）

最新一次三平台 CI matrix + aggregate 已通过：
1. workflow run_id：`22722936354`
2. 逻辑 run_id：`cross-platform-v1-22722936354`
3. 三平台 collect 全成功：`collect-ubuntu-latest`、`collect-macos-latest`、`collect-windows-latest`
4. `aggregate` 全成功并上传汇总报告
5. 结论：`DOD-11` 达标（跨平台一致性通过）

CI run 证据：
1. `https://github.com/okooo5km/pngoptim/actions/runs/22722936354`
2. `https://github.com/okooo5km/pngoptim/actions/runs/22722936354/job/65889883098`
3. `https://github.com/okooo5km/pngoptim/actions/runs/22722936354/job/65889883131`
4. `https://github.com/okooo5km/pngoptim/actions/runs/22722936354/job/65889883060`
5. `https://github.com/okooo5km/pngoptim/actions/runs/22722936354/job/65890214837`

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
   - compatibility I/O 全通过（exit-code 差异默认 advisory，可用 `--strict-compat-exit` 升级为硬门禁）
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
2. CI 三平台收口：完成（DOD-11 达标）。
