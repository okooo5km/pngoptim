# Dataset Layout v1

> 目的：固定数据集结构，避免样本漂移导致评测失真。  
> 关联条款：`EVALUATION_SPEC.md` 2.x、7.x

## 目录结构

- `dataset/functional/`: 参数行为与退出码样本
- `dataset/quality/`: 质量样本（照片/UI/透明/渐变/低色/噪声）
- `dataset/perf/`: 性能样本（大图/批量/复杂图）
- `dataset/robustness/`: 损坏与异常输入样本

## 样本元信息要求

每个样本必须能追溯以下信息（建议放在同目录 `manifest.json`）：

1. `id`
2. `filename`
3. `resolution`
4. `scene_tags`
5. `source`
6. `added_at`
7. `added_reason`

## 变更规则

1. 不允许静默替换既有样本。
2. 新增样本必须记录原因和影响评估。
3. 基线评测 run 必须记录所用样本集版本。

