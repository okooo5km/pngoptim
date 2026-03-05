# Stability Report v1

> 更新日期：2026-03-05  
> 阶段：F（稳定性与跨平台收口）  
> 主运行证据：`reports/stability/stability-v1-20260305-f1/`

## 1. 结论摘要

1. 总 case：`49`
2. regression：`9`
3. fuzz：`40`
4. crash_like（timeout/panic/signal）：`0`
5. failures：`0`

判定：当前本地环境下，回归与变异 fuzz 均达到“零崩溃”目标（DOD-10）。

## 2. 覆盖范围

1. regression：`functional/quality/perf/robustness` 全 manifest 样本。
2. fuzz：基于有效 PNG 样本生成确定性变异（截断、位翻转、块覆盖、切片复制、噪声追加）。
3. 每个 case 均设置超时门限，记录 panic/signal/timeout。

## 3. 脚本与产物

命令：
1. `cargo run --release --bin xtask -- stability`

产物：
1. `reports/stability/stability-v1-20260305-f1/stability_report.csv`
2. `reports/stability/stability-v1-20260305-f1/fuzz_summary.json`
3. `reports/stability/stability-v1-20260305-f1/failures.json`
4. `reports/stability/stability-v1-20260305-f1/summary.md`

## 4. 参数快照

1. `seed=20260305`
2. `fuzz_cases=40`
3. `timeout_sec=6`
4. 二进制：`target/release/pngoptim`
