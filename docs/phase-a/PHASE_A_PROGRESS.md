# Phase A Progress

> 阶段：A（治理与基线先行）  
> 更新日期：2026-03-05

## A1. 方案冻结

- [x] 主文档与子文档已存在并可追溯
- [x] AGENTS 阶段记忆已建立
- [x] 建立正式文档变更审批流程（`docs/phase-a/DOC_CHANGE_PROCESS_V1.md` + `.github/pull_request_template.md`）

## A2. 合规治理

- [x] 合规策略：`docs/phase-a/COMPLIANCE_POLICY_V1.md`
- [x] 依赖登记模板：`config/compliance/dependency_registry_v1.toml`
- [x] CI 合规门禁（cargo deny）落地：`.github/workflows/phase-a-governance.yml`
- [x] 本地合规检查证据：`reports/compliance/cargo-deny-check.txt`

## A3. 评测体系骨架

- [x] 数据集目录结构：`dataset/`
- [x] 首批样本清单：`dataset/functional/manifest.json`（2 个样本）
- [x] 参数矩阵：`config/evaluation/parameter_matrix_v1.toml`
- [x] 报告生成脚本：`scripts/baseline/run_baseline_v1.py`
- [x] 报告 contract：`docs/phase-a/BASELINE_REPORT_CONTRACT_V1.md`
- [x] 数据集扩充脚本：`scripts/dataset/seed_phase_a_dataset.py`

## A4. 基线产出

- [x] 参考工具版本锁定与远程可达性验证
- [x] Baseline 清单报告：`reports/baseline/BASELINE_REPORT_V1.md`
- [x] 首轮 baseline 跑数（Q_MED, functional）：`reports/baseline/baseline-20260305-qmed-fixed/`
- [x] 完整 baseline 指标报告（size/quality/perf）：`reports/baseline/baseline-v1-20260305-r2/`

## 阶段结论

- [x] 阶段 A 出口条件已满足：`Compliance Policy v1` + `Baseline Report v1` + `Acceptance Matrix v1`
