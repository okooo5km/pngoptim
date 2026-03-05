# Compliance Policy v1

> 状态：Draft-Active（阶段 A 执行中）  
> 生效日期：2026-03-05  
> 关联条款：`RECONSTRUCTION_MASTER_PLAN.md` 3.x、8.A、DOD-12

## 1. 目标

在实现功能前建立统一依赖合规门禁，确保本项目可开源、可商用、可持续维护。

## 2. 适用范围

1. Rust crates（直接依赖与传递依赖）。
2. 构建/测试工具链依赖。
3. 未来可能接入的原生库、二进制工具。

## 3. 许可证准入策略

### 3.1 默认允许（Allow）

1. MIT
2. BSD-2-Clause / BSD-3-Clause
3. Apache-2.0
4. ISC
5. Zlib

### 3.2 条件允许（Needs Review）

1. MPL-2.0（需确认文件级 copyleft 影响）
2. Unicode-DFS-2016 等特殊文本许可证（需补充归属说明）
3. 双许可证（需确认最终选择与兼容性）

### 3.3 默认禁止（Deny）

1. GPL-2.0 / GPL-3.0
2. AGPL-3.0
3. LGPL（静态链接场景）
4. SSPL
5. 带商业限制或不可再分发条款的许可证

## 4. 依赖准入最小信息

每个依赖在准入前必须记录：

1. 名称与版本（锁定策略）
2. 来源（crates.io / git / 本地镜像）
3. SPDX 许可证
4. copyleft 风险判断
5. 审批状态（approved / review / denied）
6. 责任人
7. 备注（替代方案与迁移成本）

记录位置：`config/compliance/dependency_registry_v1.toml`

## 5. 合规门禁（CI 目标）

1. `cargo deny` 许可证检查
2. `cargo deny` 安全通告与重复依赖检查
3. 自动导出第三方许可证清单（SBOM/License Report）
4. 任何 deny 级问题直接阻断合并

已落地文件：

1. 配置：`config/compliance/deny.toml`
2. 本地执行脚本：`scripts/compliance/run_compliance_checks.sh`
3. CI：`.github/workflows/phase-a-governance.yml`
4. 最近检查证据：`reports/compliance/cargo-deny-check.txt`

## 6. 豁免机制

P1/P2 风险可临时豁免，但必须同时满足：

1. 给出豁免条款 ID 与风险级别
2. 给出影响范围与临时缓解方案
3. 给出关闭条件与截止版本

豁免记录路径：`reports/waivers/<ID>.md`

## 7. 阶段 A 验收映射

1. A2（合规治理）启动条件：本文件 + registry 模板已提交
2. A2 完成条件：门禁脚本可执行且有一次通过记录
