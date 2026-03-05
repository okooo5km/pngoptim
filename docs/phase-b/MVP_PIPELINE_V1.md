# MVP Pipeline v1 (Phase B)

> 更新日期：2026-03-05  
> 目标：完成最小可运行闭环（读取 PNG -> 量化 -> 写出 PNG）

## 1. 当前实现范围

1. 单文件输入输出（PNG -> PNG）。
2. 支持基础参数：
   - `--output`
   - `--quality min-max`
   - `--speed`
   - `--nofs`
   - `--force`
   - `--skip-if-larger`
3. 量化策略：
   - 基于 `quality/speed` 的快速分层量化
   - 可选 Floyd-Steinberg 抖动（默认开，`--nofs` 关闭）
4. 写出策略：
   - PNG 编码（best compression + adaptive filter）
   - 支持 `--skip-if-larger` 保护

## 2. 退出码框架（Phase B）

1. `0`: 成功
2. `2`: 参数/输入校验错误
3. `3`: I/O 错误
4. `4`: 解码或编码失败

## 3. 代码结构

1. `src/cli.rs`: 参数定义与校验
2. `src/pipeline.rs`: 端到端流程（读取/量化/写出）
3. `src/quant.rs`: 最小量化算法
4. `src/error.rs`: 统一错误和退出码映射
5. `src/main.rs`: CLI 入口与错误处理

## 4. 执行示例

```bash
cargo run -- dataset/functional/pngquant_test.png --output /tmp/out.png --quality 60-85 --speed 4 --force
```

