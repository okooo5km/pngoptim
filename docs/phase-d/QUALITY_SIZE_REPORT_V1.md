# Quality & Size Report v1

> 更新日期：2026-03-05  
> 阶段：D（质量与体积复刻）  
> 主运行证据：`reports/quality-size/quality-size-v1-20260305-r3/`

## 1. 结论摘要

1. 样本总数：`7`
2. 失败数：`0`
3. 体积对比（candidate vs baseline）：
   - 平均：`-0.470220`
   - 中位数：`-0.435157`
   - P95：`-0.123669`
4. 质量对比（candidate - baseline）：
   - 平均 PSNR 增量：`+8.465793`
   - 平均 SSIM 增量：`+0.143980`

判定：当前数据下，阶段 D 的质量与体积目标达成（DOD-05/06/07）。

## 2. 本轮关键优化（针对高回归样本）

实现位置：`src/pipeline.rs`

1. Indexed PNG 输出改为位深自适应（1/2/4/8-bit）。
2. 新增行数据按位打包逻辑，保证低色图不再固定 8-bit 存储。
3. 编码阶段尝试多过滤器并择优（`MinEntropy/Adaptive/NoFilter/Sub/Up`）。
4. 透明调色板 `tRNS` 按最后非 255 alpha 进行裁剪，减少冗余 chunk 大小。

## 3. 回归样本闭环（r2 -> r3）

1. `perf-001-large-gradient-noise`：`+0.541798 -> -0.215607`（改善 `0.757405`）
2. `quality-001-gradient-photo`：`+0.499372 -> -0.214357`（改善 `0.713729`）
3. `quality-002-ui-alpha-icon`：`+0.314906 -> -0.123669`（改善 `0.438575`）

## 4. 产物清单

1. `reports/quality-size/quality-size-v1-20260305-r3/summary.md`
2. `reports/quality-size/quality-size-v1-20260305-r3/size_report.csv`
3. `reports/quality-size/quality-size-v1-20260305-r3/quality_report.csv`
4. `reports/quality-size/quality-size-v1-20260305-r3/top_regressions.json`
5. `reports/quality-size/quality-size-v1-20260305-r3/failures.json`

## 5. 回归稳定性验证

1. smoke：`reports/smoke/smoke-v1-20260305-d-encoding/summary.md`（9/9 通过）
2. compatibility：`reports/compat/compat-v1-20260305-d-encoding/summary.md`（参数/退出码/I/O/metadata 全通过）
