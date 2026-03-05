# Compatibility Report v1

> 更新日期：2026-03-05  
> 阶段：C（行为语义复刻）  
> 运行证据：`reports/compat/compat-v1-20260305/`

## 1. 参数兼容覆盖（C1）

检查项：`quality/speed/dither(output nofs+floyd)/output/ext/strip/skip-if-larger/posterize`

结果：

1. 覆盖率：`100.0%`（9/9）
2. 证据：`reports/compat/compat-v1-20260305/args_coverage.json`

## 2. 退出码语义（C2）

已验证映射：

1. 成功：`0`
2. 参数错误：`2`
3. I/O 失败：`3`
4. 质量不足：`98`
5. 体积不降（skip-if-larger）：`99`

结果：全部通过  
证据：`reports/compat/compat-v1-20260305/exit_codes.json`

## 3. I/O 行为（C3）

已验证：

1. 文件输入输出：通过
2. stdin/stdout：通过
3. 多文件批处理 + `--ext`：通过
4. 覆盖策略（无 `--force` 拒绝覆盖）：通过

证据：`reports/compat/compat-v1-20260305/io_behavior.json`

## 4. 元数据策略（C4）

当前状态：

1. `--strip` 参数已支持（可用）。
2. preserve 模式已实现：默认保留 text/gamma/srgb/pixel_dims/exif/icc 等 metadata。
3. 验证结果：自定义 `Comment` 文本 metadata 在 preserve 模式保留，在 `--strip` 模式被移除。

## 5. 结论

1. C1/C2/C3/C4 均通过当前兼容性门禁。
2. 阶段 C 出口条件达成，可切换为 `Done`。
