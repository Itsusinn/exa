# scripts/ 硬性要求

本文件适用于 `scripts/` 及其所有子目录，同时遵循仓库根目录的 `AGENTS.md`。

- **所有可执行脚本必须使用 Rust Cargo script。** 统一通过 `cargo +nightly -Zscript scripts/<name>.rs` 运行；这是硬性要求，不是建议。
- 禁止新增或恢复 PowerShell、Python、Shell、批处理等其他语言的脚本入口，也不能用 Cargo script 包装这些脚本来规避要求。调用 Cargo、Godot、Java 等实际工具不受此限制。
- 脚本使用内嵌 Cargo manifest；可复用 Rust 模块放在 `scripts/support/`，辅助模块无需作为独立脚本运行。
- nightly 仅用于 Cargo script；调用项目 Cargo 时遵循仓库 stable 工具链。大型 Cargo 操作串行运行并使用 `--jobs 2`。
- 新增或修改脚本时同步维护 `justfile` 的运行、格式化与检查入口，以及相关文档。脚本代码变更执行 `just scripts-check` 和对应行为验证；仅修改本文档时检查 Markdown 与差异即可。
