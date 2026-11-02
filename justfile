set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]
export CARGO_BUILD_JOBS := "2"

# 列出所有常用命令
default:
    @just --list

# 启动独立 Rust 服务端
server:
    cargo run -p server --jobs 2

# 构建扩展并启动 Godot 客户端
client:
    cargo +nightly -Zscript scripts/godot.rs run

# 同时启动独立服务端与客户端，客户端退出后保存并关闭服务端
dev:
    cargo +nightly -Zscript scripts/dev.rs

# 启动离线漫游
demo:
    cargo +nightly -Zscript scripts/godot.rs demo

# 构建整个工作区并安装 Godot 扩展 DLL
build:
    cargo build --workspace --jobs 2
    cargo +nightly -Zscript scripts/godot.rs build

# 格式化所有 Rust 代码
fmt:
    cargo fmt --all
    rustup run nightly rustfmt --edition 2024 scripts/godot.rs scripts/godot-mcp.rs scripts/dev.rs scripts/thermo_oracle.rs

# 检查 Rust 格式，不修改文件
fmt-check:
    cargo fmt --all -- --check

# 严格检查全部 Rust target，警告视为错误
clippy:
    cargo clippy --workspace --all-targets --jobs 2 -- -D warnings

# 依次执行格式检查和严格 Clippy
check: fmt-check clippy scripts-check

# 检查所有 Cargo script 入口；不会启动 Godot 窗口或服务
scripts-check:
    rustup run nightly rustfmt --check --edition 2024 scripts/godot.rs scripts/godot-mcp.rs scripts/dev.rs scripts/thermo_oracle.rs
    cargo +nightly clippy -Zscript --manifest-path scripts/godot.rs --all-targets --jobs 2 -- -D warnings
    cargo +nightly clippy -Zscript --manifest-path scripts/godot-mcp.rs --all-targets --jobs 2 -- -D warnings
    cargo +nightly clippy -Zscript --manifest-path scripts/dev.rs --all-targets --jobs 2 -- -D warnings
    cargo +nightly clippy -Zscript --manifest-path scripts/thermo_oracle.rs --all-targets --jobs 2 -- -D warnings

# 验证运行时导出过滤、脚本文件复制及 DLL 比较
scripts-test:
    cargo +nightly -Zscript test --manifest-path scripts/godot.rs --jobs 2

# 运行工作区全部 Rust 测试
test:
    cargo test --workspace --jobs 2

# 单独运行真实 QUIC 多客户端集成测试
test-multiplayer:
    cargo test -p client --test multiplayer --jobs 2

# 生成工作区 API 文档（不生成依赖文档）
doc:
    cargo doc --workspace --no-deps --jobs 2

# 清理 Cargo 构建缓存；不清理 Godot 缓存或已安装的 DLL
clean:
    cargo clean

# 构建扩展并打开正式项目编辑器
editor:
    cargo +nightly -Zscript scripts/godot.rs editor

# 安装/构建可选 MCP 插件并启动无窗口编辑器
godot-mcp:
    cargo +nightly -Zscript scripts/godot-mcp.rs

# 检查 Godot 导入、扩展加载和离线场景
godot-smoke:
    cargo +nightly -Zscript scripts/godot.rs smoke

# 使用隔离服务端检查两个 Godot 桥接实例联机
godot-multiplayer:
    cargo +nightly -Zscript scripts/godot.rs multiplayer

# 使用隔离世界验证箱子 UI、存取和保存重开
godot-containers:
    cargo +nightly -Zscript scripts/godot.rs containers

# 使用隔离世界验证方块破坏、放置、UI 输入隔离和保存重开
godot-editing:
    cargo +nightly -Zscript scripts/godot.rs editing

# 使用隔离世界验证工业设备建造、旋转、碰撞、检查与保存重开
godot-industry:
    cargo +nightly -Zscript scripts/godot.rs industry

# 使用实际 Forward+ 窗口检查工业设备、预览与面板截图
godot-industry-graphics:
    cargo +nightly -Zscript scripts/godot.rs industry-graphics

# 验证最小工业回路、控制面板与保存重开
godot-industry-flow:
    cargo +nightly -Zscript scripts/godot.rs industry-flow

# 在实际 Forward+ 窗口验证工业运行读数与控制
godot-industry-flow-graphics:
    cargo +nightly -Zscript scripts/godot.rs industry-flow-graphics

# 验证气液相态、双流换热、控制同步与保存重开
godot-industry-thermal:
    cargo +nightly -Zscript scripts/godot.rs industry-thermal

# 在实际 Forward+ 窗口验证密闭罐与双流换热器
godot-industry-thermal-graphics:
    cargo +nightly -Zscript scripts/godot.rs industry-thermal-graphics

# 验证原子铺管、弯头、管架及跨高度回路
godot-industry-route:
    cargo +nightly -Zscript scripts/godot.rs industry-route

# 在实际 Forward+ 窗口验证整条路线预览和弯管回路
godot-industry-route-graphics:
    cargo +nightly -Zscript scripts/godot.rs industry-route-graphics

# Parameterized tanks and request-only heat unit inspection.
godot-industry-design:
    cargo +nightly -Zscript scripts/godot.rs industry-design

godot-industry-design-graphics:
    cargo +nightly -Zscript scripts/godot.rs industry-design-graphics

# Binary steady-state column calculator; no material is taken from the world.
godot-distillation:
    cargo +nightly -Zscript scripts/godot.rs distillation

godot-distillation-graphics:
    cargo +nightly -Zscript scripts/godot.rs distillation-graphics

# 验证蒸馏设计导入、塔体建造和三罐外部接管
godot-column:
    cargo +nightly -Zscript scripts/godot.rs column

# 实际图形窗口验收参数化塔体和设计面板
godot-column-graphics:
    cargo +nightly -Zscript scripts/godot.rs column-graphics

# 真实 QUIC 探索后在图形窗口验证负坐标区块加载、卸载与保存
godot-streaming:
    cargo +nightly -Zscript scripts/godot.rs streaming

# 实际图形窗口测量新区块加载耗时并检查地形队列清理
godot-streaming-perf:
    cargo +nightly -Zscript scripts/godot.rs streaming-perf

# 实际图形窗口验证六类生态、128 格高度、保存重开与区块卸载
godot-ecology:
    cargo +nightly -Zscript scripts/godot.rs ecology

# 实际图形窗口验证村庄、城堡、正负坐标区块与保存重开
godot-structures:
    cargo +nightly -Zscript scripts/godot.rs structures

# 实际图形窗口验证自然出生地形、移动、返回出生点和保存重开
godot-natural-spawn:
    cargo +nightly -Zscript scripts/godot.rs natural-spawn

# 使用隔离世界验证聊天指令、传送、飞行与输入隔离
godot-commands:
    cargo +nightly -Zscript scripts/godot.rs commands

# 导出 Windows 调试客户端
export-debug:
    cargo +nightly -Zscript scripts/godot.rs export-debug

# 导出 Windows 发布客户端
export-release:
    cargo +nightly -Zscript scripts/godot.rs export-release
