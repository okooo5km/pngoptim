# RC Candidate v1

> 更新日期：2026-03-06  
> 阶段：F（稳定性与跨平台收口）

## 1. 目标

确认发布候选门禁满足阶段 F 出口条件：
1. 稳定性达标（DOD-10）
2. 跨平台一致性达标（DOD-11）
3. 发布检查链路可执行（RC 规则）

## 2. 门禁核验结果

1. 稳定性：通过  
   证据：`docs/phase-f/STABILITY_REPORT_V1.md`
2. 跨平台一致性：通过  
   证据：`docs/phase-f/CROSS_PLATFORM_REPORT_V1.md`
3. 发布检查命令：通过  
   命令：`cargo run --release --bin xtask -- release-check --run-id release-check-local-rust-final`

## 3. CI 收口证据

1. workflow：`phase-f-cross-platform`
2. run_id：`22722936354`
3. 结果：`success`
4. 链接：`https://github.com/okooo5km/pngoptim/actions/runs/22722936354`

## 4. 结论

`RC Candidate v1` 就绪，可进入阶段 G 主线任务。
