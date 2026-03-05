# 评测规范（Evaluation Spec）v1

> 关联主文档：`RECONSTRUCTION_MASTER_PLAN.md`  
> 目标：统一“体积/质量/性能/稳定性”评测方法，避免主观争议。

## 参考开源项目与本地仓库

1. **kornelski/pngquant**  
   - GitHub: https://github.com/kornelski/pngquant  
   - 本地绝对路径: `/Users/5km/Dev/C/pngquant`
2. **ImageOptim/libimagequant**  
   - GitHub: https://github.com/ImageOptim/libimagequant  
   - 本地绝对路径: `/Users/5km/Dev/C/libimagequant`

---

## 1. 评测总原则

1. 所有比较必须基于同一输入集与同一参数组。
2. 评测输出必须结构化（CSV/JSON），可复现。
3. 一切结论以报告数据为准，不以主观截图争论。

---

## 2. 数据集规范

## 2.1 分层
1. `dataset/functional/`：参数与行为测试样本
2. `dataset/quality/`：画质样本（照片、UI、透明、渐变、低色、噪声）
3. `dataset/perf/`：性能样本（大图、批量、极端图）
4. `dataset/robustness/`：异常与损坏样本

## 2.2 样本要求
- 每类样本均需包含：`id`、分辨率、场景标签、来源说明
- 禁止随机替换样本导致基线漂移
- 新增样本必须记录变更原因

---

## 3. 指标定义

## 3.1 体积指标
- `size_bytes_out`
- `size_ratio = out / in`
- `delta_vs_baseline = (candidate - baseline) / baseline`

统计口径：均值、 中位数、P90、P95

## 3.2 质量指标
- 主指标：建议 `SSIM` 或 `Butteraugli`
- 辅指标：`PSNR`（必要时附 LPIPS）
- 质量门禁按样本类别设置阈值（照片与 UI 可不同）

## 3.3 性能指标
- 单图耗时（ms）
- 批量吞吐（img/s）
- CPU 占用与线程效率（可选）

## 3.4 资源指标
- 峰值 RSS 内存
- 峰值线程数

## 3.5 稳定性指标
- 崩溃率
- 超时率
- 失败率（非预期）

---

## 4. 参数组规范（测试矩阵）

定义固定参数档：
1. `Q_HIGH`：高质量档（如 quality 70-90）
2. `Q_MED`：均衡档
3. `Q_LOW`：高压缩档
4. `FAST`：高速度档
5. `NO_DITHER`：禁抖动专项

> 参数矩阵必须冻结在版本控制中，禁止临时改参数“刷分”。

---

## 5. 对比规范

## 5.1 基线定义
- Baseline 工具版本固定（commit/tag）
- 测试环境固定（OS/CPU/线程配置）

## 5.2 候选对比
- Candidate 与 Baseline 逐样本对比
- 输出差异文件：
  - `size_report.csv`
  - `quality_report.csv`
  - `perf_report.csv`

## 5.3 判定规则
- 达标：全部 P0 条款通过
- 预警：P1 出现退化但在豁免范围
- 失败：任一 P0 条款失败

---

## 6. 报告结构规范

每次里程碑报告包含：
1. 执行摘要（通过/失败）
2. 总体指标表
3. Top 退化样本列表（含可重放参数）
4. 根因归因（模块级）
5. 建议动作（修复/豁免/回滚）

建议目录：
- `reports/<run_id>/summary.md`
- `reports/<run_id>/size_report.csv`
- `reports/<run_id>/quality_report.csv`
- `reports/<run_id>/perf_report.csv`
- `reports/<run_id>/failures.json`

---

## 7. 可复现规范

1. 锁定输入数据集版本
2. 锁定测试参数矩阵版本
3. 锁定编译配置版本
4. 锁定机器信息快照
5. 每次 run 生成唯一 `run_id`

---

## 8. 稳定性与鲁棒性规范

1. 回归测试必须全量跑过
2. fuzz 测试定期跑并存档
3. 损坏输入不得崩溃，应输出明确错误
4. 极端大图应可控失败（内存/时间保护）

---

## 9. 争议处理机制

当“视觉看起来更好/更差”发生争议时：
1. 先看指标结果
2. 再看同一裁剪区域对比图
3. 最后由评审组按项目目标决策

> 默认以指标与可复现证据为准。
