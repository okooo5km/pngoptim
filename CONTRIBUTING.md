# Contributing Guide

## 1. Scope

本项目采用“文档先行 + 指标驱动”的协作方式。所有变更都必须可验证、可回滚、可复现。

## 2. Branch & Commit

1. 分支命名使用前缀：`codex/`
2. 每次提交只做一类改动（行为、质量、性能、稳定性、文档）
3. 禁止提交无法复现的临时调参

## 3. PR Requirements

每个 PR 必须填写：
1. `Phase`
2. `DoD IDs`
3. `Report/Test paths`

模板见：`.github/pull_request_template.md`

## 4. Local Validation

提交前至少运行：
```bash
cargo fmt
cargo check
cargo test
```

按变更类型补充：
```bash
python3 scripts/smoke/run_smoke_phase_b.py --run-id smoke-local --binary target/release/pngoptim
python3 scripts/compat/run_phase_c_compat.py --run-id compat-local --binary target/release/pngoptim
python3 scripts/evaluation/run_phase_d_quality_size.py --run-id quality-size-local --candidate target/release/pngoptim
python3 scripts/perf/run_phase_e_perf.py --run-id perf-local --candidate target/release/pngoptim
python3 scripts/stability/run_phase_f_stability.py --run-id stability-local --binary target/release/pngoptim
```

## 5. Data & Reports

1. 不允许“无数据优化”
2. 影响行为/指标的改动必须附报告路径
3. 新增数据样本必须更新对应 `dataset/*/manifest.json`
4. 样本来源与版权必须写明

## 6. Dependency & License

1. 新依赖需符合 `config/compliance/deny.toml`
2. 新依赖需更新 `config/compliance/dependency_registry_v1.toml`
3. 合规门禁失败不可合并

## 7. Communication

1. 问题定位优先给最小可重放命令
2. 性能争议优先看结构化报告，不做主观截图争论
