# 验收矩阵（Acceptance Matrix）v1

> 关联主文档：`RECONSTRUCTION_MASTER_PLAN.md`  
> 用途：将 DoD 转成可执行检查表，供编码 Agent 与评审统一使用。

## 参考开源项目与本地仓库

1. **kornelski/pngquant**  
   - GitHub: https://github.com/kornelski/pngquant  
   - 本地绝对路径: `/Users/5km/Dev/C/pngquant`
2. **ImageOptim/libimagequant**  
   - GitHub: https://github.com/ImageOptim/libimagequant  
   - 本地绝对路径: `/Users/5km/Dev/C/libimagequant`

---

## 1. 使用规则

1. 每次里程碑交付必须附本矩阵打分。
2. 任一 **P0 条款失败 = 不可合并**。
3. P1 条款可临时豁免，但必须登记风险与修复计划。

---

## 2. 验收项总览

| ID | 验收项 | 优先级 | 验收方式 | 通过标准 |
|---|---|---|---|---|
| DOD-01 | 参数兼容覆盖率 | P0 | 自动测试 | 覆盖率 ≥ 95% |
| DOD-02 | 退出码语义一致 | P0 | 自动测试 | 核心错误码全通过 |
| DOD-03 | 输入输出行为一致 | P0 | 自动测试 | 文件/流模式均通过 |
| DOD-04 | 元数据策略稳定 | P1 | 自动+人工抽检 | strip/保留无异常 |
| DOD-05 | 体积等价/更优 | P0 | 基准评测 | 平均体积不劣于阈值 |
| DOD-06 | 视觉质量达标 | P0 | 质量评测 | 指标不低于阈值 |
| DOD-07 | 透明/低色专项 | P0 | 专项样本评测 | 不出现明显劣化 |
| DOD-08 | 性能不退化 | P1 | 性能评测 | 至少一项优于基线 |
| DOD-09 | 内存峰值受控 | P1 | 资源监测 | 峰值不超上限 |
| DOD-10 | 稳定性 | P0 | 回归+fuzz | 无崩溃 |
| DOD-11 | 跨平台一致性 | P1 | 三平台回归 | 统计指标一致 |
| DOD-12 | 可复现性 | P0 | 重跑验证 | 结果稳定可重放 |

---

## 3. 逐项检查模板

## DOD-01 参数兼容覆盖率（P0）
- [ ] 参数列表冻结（含默认值）
- [ ] 解析行为一致
- [ ] 组合参数行为一致
- [ ] 覆盖率报告达标（≥95%）
- 证据：`reports/compat/args_coverage.json`

## DOD-02 退出码语义一致（P0）
- [ ] 成功码
- [ ] 参数错误码
- [ ] 质量不足码
- [ ] 体积不降码
- [ ] I/O 失败码
- 证据：`reports/compat/exit_codes.json`

## DOD-03 输入输出行为一致（P0）
- [ ] 文件输入输出
- [ ] stdin/stdout
- [ ] 批量处理
- [ ] 覆盖与目标路径策略
- 证据：`reports/compat/io_behavior.json`

## DOD-04 元数据策略稳定（P1）
- [ ] `--strip` 生效
- [ ] 保留模式不损坏 metadata
- [ ] 色彩相关 chunk 行为符合策略
- 证据：`reports/quality/metadata_cases.json`

## DOD-05 体积等价/更优（P0）
- [ ] 平均体积差 <= 阈值
- [ ] 中位数体积差 <= 阈值
- [ ] P95 不出现异常劣化
- 证据：`reports/benchmark/size_report.csv`

## DOD-06 视觉质量达标（P0）
- [ ] 主指标达标（SSIM 或 Butteraugli）
- [ ] 辅指标达标（PSNR/另一项）
- [ ] 人工抽检无明显退化
- 证据：`reports/benchmark/quality_report.csv`

## DOD-07 透明/低色专项（P0）
- [ ] 低色图（16/32/64）达标
- [ ] 半透明边缘图达标
- [ ] UI/icon 样本达标
- 证据：`reports/special/alpha_lowcolor.json`

## DOD-08 性能不退化（P1）
- [ ] 单线程耗时对比
- [ ] 多线程吞吐对比
- [ ] 至少一项有正收益
- 证据：`reports/perf/perf_compare.csv`

## DOD-09 内存峰值受控（P1）
- [ ] 典型图峰值内存达标
- [ ] 大图峰值内存达标
- [ ] 无异常增长
- 证据：`reports/perf/memory_profile.json`

## DOD-10 稳定性（P0）
- [ ] 回归集零崩溃
- [ ] fuzz 零崩溃
- [ ] 错误可恢复
- 证据：`reports/stability/fuzz_summary.json`

## DOD-11 跨平台一致性（P1）
- [ ] macOS 指标达标
- [ ] Linux 指标达标
- [ ] Windows 指标达标
- [ ] 差异在阈值内
- 证据：`reports/cross_platform/consistency.csv`

## DOD-12 可复现性（P0）
- [ ] 固定输入重跑一致
- [ ] 固定版本重跑一致
- [ ] CI 可重放
- 证据：`reports/repro/repro_check.json`

---

## 4. 里程碑验收门槛

- **MVP 通过条件**：DOD-01/02/03/10/12
- **复刻通过条件**：MVP + DOD-05/06/07
- **发布候选通过条件**：复刻通过 + DOD-08/09/11

---

## 5. 风险豁免机制

豁免必须包含：
1. 豁免条款 ID
2. 风险等级
3. 影响范围
4. 临时措施
5. 关闭条件（何时撤销豁免）

模板文件建议：`reports/waivers/<ID>.md`
