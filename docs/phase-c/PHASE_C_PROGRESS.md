# Phase C Progress

> 阶段：C（行为语义复刻）  
> 更新日期：2026-03-05

## C1. 参数对齐

- [x] quality/speed/dither/output/ext
- [x] strip/skip-if-larger/posterize
- [x] 参数覆盖报告：`reports/compat/compat-v1-20260305/args_coverage.json`

## C2. 退出码与错误语义

- [x] 成功码：0
- [x] 参数错误码：2
- [x] 质量不足码：98
- [x] 体积不降码：99
- [x] I/O 失败码：3
- [x] 退出码报告：`reports/compat/compat-v1-20260305/exit_codes.json`

## C3. I/O 行为

- [x] stdin/stdout
- [x] 批处理
- [x] 覆盖策略
- [x] I/O 报告：`reports/compat/compat-v1-20260305/io_behavior.json`

## C4. 元数据策略

- [x] `--strip` 参数可用
- [x] 保留模式（metadata preserve）已实现并验证

## 阶段结论

- [x] 阶段 C 出口条件已满足：`Compatibility Report v1` + 行为差异清单（当前无未收口差异）
