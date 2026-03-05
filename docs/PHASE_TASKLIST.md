# 阶段任务清单（Phase Task List）v1

> 关联主文档：`RECONSTRUCTION_MASTER_PLAN.md`  
> 目的：给编码 Agent 提供按顺序执行的任务骨架（不设工期）。

## 参考开源项目与本地仓库

1. **kornelski/pngquant**  
   - GitHub: https://github.com/kornelski/pngquant  
   - 本地绝对路径: `/Users/5km/Dev/C/pngquant`
2. **ImageOptim/libimagequant**  
   - GitHub: https://github.com/ImageOptim/libimagequant  
   - 本地绝对路径: `/Users/5km/Dev/C/libimagequant`

---

## 阶段 A：治理与基线先行

### A1. 方案冻结
- [ ] 冻结主文档版本号
- [ ] 建立变更审查机制（文档先于实现）

### A2. 合规治理
- [ ] 建立依赖准入清单模板
- [ ] 建立许可证审查流程
- [ ] 定义合规 CI 门禁

### A3. 评测体系骨架
- [ ] 定义数据集目录结构
- [ ] 定义参数矩阵与固定阈值
- [ ] 定义报告格式

### A4. 基线产出
- [ ] 跑出 baseline 初始报告
- [ ] 冻结 baseline 版本

**阶段出口条件**：`Baseline Report v1` + `Compliance Policy v1` 完成

---

## 阶段 B：最小可运行闭环

### B1. 端到端链路
- [ ] 读取 PNG
- [ ] 最小量化流程
- [ ] 写出 PNG

### B2. CLI 初版
- [ ] 单文件处理
- [ ] 基础参数入口
- [ ] 错误码框架

### B3. 稳定性底线
- [ ] smoke 样本全通过
- [ ] 无崩溃

**阶段出口条件**：MVP 可跑全样本 smoke

---

## 阶段 C：行为语义复刻

### C1. 参数对齐
- [ ] quality/speed/dither/output/ext
- [ ] strip/skip-if-larger/posterize

### C2. 退出码与错误语义
- [ ] 核心错误码对齐
- [ ] 错误文案可诊断

### C3. I/O 行为
- [ ] stdin/stdout
- [ ] 批处理
- [ ] 覆盖策略

### C4. 元数据策略
- [ ] strip 行为
- [ ] 保留行为

**阶段出口条件**：`Compatibility Report v1` 达标

---

## 阶段 D：质量与体积复刻

### D1. 质量对齐
- [ ] 主指标达标
- [ ] 辅指标达标

### D2. 体积对齐
- [ ] 均值/中位数/P95 达标

### D3. 专项修复
- [ ] 透明边缘样本
- [ ] 低色样本
- [ ] UI/渐变样本

### D4. 失败样本闭环
- [ ] 建立 top 退化样本清单
- [ ] 每轮清零/下降

**阶段出口条件**：`Quality & Size Report v1` 达标

---

## 阶段 E：性能优化冲刺

### E1. 可观测性先行
- [ ] 模块级耗时分解
- [ ] 内存画像

### E2. 热点优化（逐项）
- [ ] 搜索路径优化
- [ ] 抖动路径优化
- [ ] 写出压缩优化

### E3. 平台优化
- [ ] SIMD 路径
- [ ] 并行调度策略

### E4. 质量守护
- [ ] 每项优化必须回归质量/体积门禁

**阶段出口条件**：`Perf Report v1` 达标

---

## 阶段 F：稳定性与跨平台收口

### F1. 鲁棒性
- [ ] 异常输入处理
- [ ] 回归零崩溃
- [ ] fuzz 零崩溃

### F2. 跨平台
- [ ] macOS 回归
- [ ] Linux 回归
- [ ] Windows 回归

### F3. 发布门禁
- [ ] 可复现构建
- [ ] RC 规则

**阶段出口条件**：`RC Candidate` 就绪

---

## 阶段 G：开源发布与协作

### G1. 发布材料
- [ ] 用户文档
- [ ] 基准与评测说明
- [ ] 许可证与依赖声明

### G2. 社区治理
- [ ] 贡献指南
- [ ] issue 模板
- [ ] PR 门禁模板

### G3. 持续演进
- [ ] 性能回归长期监控
- [ ] 样本集持续扩充机制

**阶段出口条件**：`Public Release v1`

---

## 附录：任务执行约束（给编码 Agent）

1. 每个任务必须绑定 DoD 条款编号。
2. 每次提交必须附报告路径。
3. 不允许“无数据优化”。
4. 优化必须可开关、可回滚。
5. 文档变更优先于实现变更。
