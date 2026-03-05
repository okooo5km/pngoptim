# Benchmark Reproduction v1

> 更新日期：2026-03-06  
> 适用阶段：G（开源发布）

## 1. 目标

复现阶段 D/E/F 的核心评测：质量体积、性能、稳定性与跨平台一致性。

## 2. 前置条件

1. 已执行 `cargo build --release`
2. 本地具备数据集目录 `dataset/`
3. 可执行文件存在：`target/release/pngoptim`

## 3. 单机复现实验

### 3.1 质量与体积（Phase D）

```bash
cargo run --release --bin xtask -- quality-size --run-id quality-size-repro-v1 --candidate target/release/pngoptim --quality 55-75 --speed 4
```

输出：`reports/quality-size/quality-size-repro-v1/`

### 3.2 性能（Phase E）

```bash
cargo run --release --bin xtask -- perf --run-id perf-repro-v1 --candidate target/release/pngoptim --quality 55-75 --speed 4 --iterations 2
```

输出：`reports/perf/perf-repro-v1/`

### 3.3 稳定性（Phase F）

```bash
cargo run --release --bin xtask -- stability --run-id stability-repro-v1 --binary target/release/pngoptim --fuzz-cases 24
```

输出：`reports/stability/stability-repro-v1/`

## 4. 跨平台复现（CI 推荐）

1. 使用 workflow：`.github/workflows/phase-f-cross-platform.yml`
2. 三平台并行 collect 后由 aggregate 汇总
3. 关注产物：
   - `reports/cross_platform/<run_id>/consistency.csv`
   - `reports/cross_platform/<run_id>/summary.md`

## 5. 发布材料复现

```bash
cargo run --release --bin xtask -- release-licenses --run-id release-licenses-repro-v1
cargo run --release --bin xtask -- release-check --run-id release-check-repro-v1
cargo run --release --bin xtask -- release-package --run-id public-release-v1-repro --binary target/release/pngoptim --build
```

输出：`reports/release/public-release-v1-repro/`
