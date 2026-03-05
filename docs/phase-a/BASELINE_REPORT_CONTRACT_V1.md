# Baseline Report Contract v1

> 生效日期：2026-03-05  
> 关联阶段：A3/A4  
> 生成命令：`cargo run --release --bin xtask -- baseline --run-id <run_id> --profile Q_MED`

## 1. 输出目录

每次评测必须输出到：

`reports/baseline/<run_id>/`

其中 `<run_id>` 必须唯一且可追溯。

## 2. 必需文件

1. `size_report.csv`
2. `quality_report.csv`
3. `perf_report.csv`
4. `failures.json`
5. `summary.md`
6. `run_meta.json`

## 3. 字段契约

### 3.1 size_report.csv

字段：

`run_id,profile,dataset_split,sample_id,input_file,input_bytes,output_file,output_bytes,size_ratio,delta_bytes,exit_code,expected_success,status`

口径：

1. `size_ratio = output_bytes / input_bytes`
2. `delta_bytes = output_bytes - input_bytes`
3. `status`: `success|failed`

### 3.2 quality_report.csv

字段：

`run_id,profile,dataset_split,sample_id,input_file,output_file,psnr_db,ssim,shape_match,exit_code,status`

口径：

1. `psnr_db`：基于 RGBA 的 MSE 计算。
2. `ssim`：全局 SSIM 近似值（通道平均）。
3. `shape_match=false` 时 `psnr_db/ssim` 可为空。

### 3.3 perf_report.csv

字段：

`run_id,profile,dataset_split,sample_id,input_file,elapsed_ms,exit_code,expected_success,status`

口径：

1. `elapsed_ms`：单样本 wall-clock 耗时。
2. `status`: `success|failed`

### 3.4 failures.json

记录所有异常项，包含至少：

1. `split`
2. `sample_id`
3. `filename`
4. `reason`

若为非预期结果，还应包含：

1. `expected_success`
2. `actual_success`
3. `exit_code`
4. `stderr`（截断）

### 3.5 summary.md

至少包含：

1. 运行概览（总数、成功、失败、unexpected）
2. 聚合指标（size_ratio mean/median/p95，elapsed mean/median/p95）
3. 质量均值（psnr/ssim，若有）
4. 本次输出文件路径

### 3.6 run_meta.json

至少包含：

1. `generated_at_utc`
2. `run_id`
3. `profile`
4. `splits`
5. 平台信息（system/release/machine/rust）

## 4. 判定规则（Phase A）

1. 脚本可重复执行且输出完整文件集。
2. `failures.json` 中不得存在 `unexpected_result`（若存在则需登记豁免）。
3. 运行元信息完整可追溯。
