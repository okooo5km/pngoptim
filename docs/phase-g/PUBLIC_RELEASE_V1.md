# Public Release v1（Preflight）

> 更新日期：2026-03-05  
> 阶段：G（开源发布与社区协作）  
> 状态：Preflight Ready（等待阶段 F 三平台 CI 收口）

## 1. 发布目标

1. 提供可复现的评测与回归链路。
2. 提供可执行的贡献规范与问题模板。
3. 提供第三方依赖许可证快照与发布检查。

## 2. 已落地资产

1. 贡献规范：`CONTRIBUTING.md`
2. Issue 模板：
   - `.github/ISSUE_TEMPLATE/bug_report.yml`
   - `.github/ISSUE_TEMPLATE/compat_regression.yml`
   - `.github/ISSUE_TEMPLATE/perf_regression.yml`
3. PR 模板：`.github/pull_request_template.md`
4. 定时回归 workflow：`.github/workflows/nightly-regression.yml`
5. 三平台一致性 workflow：`.github/workflows/phase-f-cross-platform.yml`
6. 许可证导出命令：`cargo run --release --bin xtask -- release-licenses`
7. 发布检查命令：`cargo run --release --bin xtask -- release-check`

## 3. 发布前必须完成项

1. 阶段 F 三平台 CI 一致性结论通过（DOD-11）。
2. 生成最新稳定性、性能、质量体积报告。
3. 生成最新第三方许可证快照。
4. 运行发布检查脚本并通过。

## 4. 建议发布命令

```bash
cargo run --release --bin xtask -- release-licenses --run-id release-licenses-v1
cargo run --release --bin xtask -- release-check --run-id release-check-v1
```
