# Public Release v1

> 更新日期：2026-03-06  
> 阶段：G（开源发布与社区协作）  
> 状态：In Progress（阶段 F 已收口，进入发布包执行）

## 1. 发布目标

1. 提供可复现的评测与回归链路。
2. 提供可执行的贡献规范与问题模板。
3. 提供第三方依赖许可证快照与发布检查。

## 2. 已落地资产

1. 贡献规范：`CONTRIBUTING.md`
2. 用户文档：`docs/phase-g/USER_GUIDE_V1.md`
3. 评测复现文档：`docs/phase-g/BENCHMARK_REPRO_V1.md`
4. 开源许可证：`LICENSE`
5. Issue 模板：
   - `.github/ISSUE_TEMPLATE/bug_report.yml`
   - `.github/ISSUE_TEMPLATE/compat_regression.yml`
   - `.github/ISSUE_TEMPLATE/perf_regression.yml`
6. PR 模板：`.github/pull_request_template.md`
7. 定时回归 workflow：`.github/workflows/nightly-regression.yml`
8. 三平台一致性 workflow：`.github/workflows/phase-f-cross-platform.yml`
9. 许可证导出命令：`cargo run --release --bin xtask -- release-licenses`
10. 发布检查命令：`cargo run --release --bin xtask -- release-check`
11. 发布打包命令：`cargo run --release --bin xtask -- release-package`

## 3. 发布前必须完成项

1. 阶段 F 三平台 CI 一致性结论通过（DOD-11）。已完成。
2. 生成最新稳定性、性能、质量体积报告。
3. 生成最新第三方许可证快照。
4. 运行发布检查脚本并通过。
5. 生成并归档 `Public Release Bundle v1`。

## 4. 建议发布命令

```bash
cargo run --release --bin xtask -- release-licenses --run-id release-licenses-v1
cargo run --release --bin xtask -- release-check --run-id release-check-v1
cargo run --release --bin xtask -- release-package --run-id public-release-v1 --binary target/release/pngoptim --build
```

## 5. 最新执行证据

1. 发布包 run_id：`public-release-v1-20260306-g1-verify`
2. 产物摘要：`reports/release/public-release-v1-20260306-g1-verify/summary.md`
3. 清单校验：`reports/release/public-release-v1-20260306-g1-verify/bundle_manifest.json`
4. 许可证快照：`reports/release/public-release-v1-20260306-g1-verify-licenses/summary.md`
5. 发布检查：`reports/release/public-release-v1-20260306-g1-verify-check/summary.md`
