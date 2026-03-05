# User Guide v1

> 更新日期：2026-03-06  
> 适用阶段：G（开源发布）

## 1. 环境要求

1. Rust stable（建议通过 `rustup` 安装）
2. 本地可用 `cargo`

## 2. 构建

```bash
cargo build --release
```

产物路径：`target/release/pngoptim`

## 3. 基本用法

### 3.1 单文件输出到指定路径

```bash
target/release/pngoptim dataset/functional/pngquant_test.png --output out.png --force
```

### 3.2 质量/速度参数

```bash
target/release/pngoptim dataset/quality/gradient_photo.png --output out.png --quality 55-75 --speed 4 --force
```

### 3.3 stdin/stdout 管道

```bash
cat dataset/functional/pngquant_test.png | target/release/pngoptim - --output - > out.png
```

### 3.4 批量输入（自动后缀输出）

```bash
target/release/pngoptim dataset/functional/pngquant_test.png dataset/functional/pngquant_metadata.png --ext=-mvp.png --force
```

## 4. 常用参数

1. `--quality min-max`：质量区间（例如 `55-75`）
2. `--speed 1..11`：编码速度档位
3. `--strip`：剥离元数据
4. `--posterize 0..8`：色阶量化
5. `--skip-if-larger`：若输出更大则失败并返回 99
6. `--force`：允许覆盖输出文件
7. `--quiet`：静默模式

## 5. 退出码约定

1. `0`：成功
2. `2`：参数或输入错误
3. `3`：I/O 错误
4. `4`：解码/编码错误
5. `98`：质量门禁失败
6. `99`：输出大于输入且启用 `--skip-if-larger`

## 6. 工程命令（xtask）

```bash
cargo run --release --bin xtask -- smoke --run-id smoke-local --binary target/release/pngoptim
cargo run --release --bin xtask -- compat --run-id compat-local --binary target/release/pngoptim
cargo run --release --bin xtask -- quality-size --run-id quality-size-local --candidate target/release/pngoptim
cargo run --release --bin xtask -- perf --run-id perf-local --candidate target/release/pngoptim
cargo run --release --bin xtask -- stability --run-id stability-local --binary target/release/pngoptim
```
