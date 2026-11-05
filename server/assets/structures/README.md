# 结构资源

两个 JSON 由 `server/src/voxel/structures.rs` 嵌入；单人和独立服务端使用同一份资源。修改后重新构建 Rust 服务端及扩展。没有运行时热加载，也不读取 Minecraft NBT。

## 添加结构

1. 在 `templates.json` 添加命名模板。`fills` 按列表顺序覆盖，`from` / `to` 是包含端点的本地 `[x,y,z]`；`block` 使用 `exa_protocol::BLOCK_IDS` 中的稳定名称。显式 `game:air` 用于门窗与内部空间；未被填充指令覆盖的位置保持整地后的状态。
2. 在 `definitions.json` 的 `structures` 中添加唯一 `id`，设置非零 `weight`、`biomes`、`max_slope`、`max_distance_from_center` 和 `pieces`。
3. 每个 piece 的 `offset` 是相对中心的 `[x,z]`，`rotation` 为 0–3 次 90° 旋转，变换为 `[x,z] -> [-z,x]`。模板池中的各项包含 `template` 引用和非零 `weight`。所有 piece 共享结构地坪，模板 y=0 对应地坪表面；结构整体还会按种子旋转。
4. 执行 `cargo test -p exa-server --jobs 2 voxel::terrain::structures`、`just check`、`just test` 和 `just godot-structures`，检查入口、跨区块拼接、种子分布、保存与截图。新增结构时同步调整测试的类型覆盖，不删除已有回归。

## 参数与限制

| 字段 | 当前支持范围 |
| --- | --- |
| `version` | 1，结构资源模式版本，不是世界存档版本 |
| `placement.spacing` | 8–128，单位为 16 格区块 |
| `placement.separation` | 4 至 spacing−1；相邻区域沿分隔方向的锚点间隔至少为 separation+1 个区块 |
| `placement.salt` | u64；参与区域锚点散列 |
| `biomes` | grassland、forest、taiga、desert、wetland、snow |
| `terrain_adaptation` | terrace；在自然地形和树木之后处理，结构内部整平，外缘 4 格渐变 |
| `max_slope` | 0–12；整片范围内 5×5 个地表采样点的最高与最低差 |
| `max_distance_from_center` | 8–32；模板变换后必须落在半径减 4 的方形内部，并保持相邻结构不重叠 |
| 结构/模板池 `weight` | 1–100；按确定性整数散列进行加权选择 |
| 模板填充坐标 | X/Z 为 −32 至 32；Y 为 0–24 |

每结构最多 64 个 piece，每个池最多 32 项，每模板最多 256 次填充；当前不支持嵌套模板池、递归 jigsaw、实体生成或箱子方块实体，所以 `game:chest` 会拒绝加载。其他非法字段、缺失模板、未知方块或越界也会在接受会话前报错。

世界仍以普通方块保存结构，不另存结构实例。生成只写当前未探索区块；不要借此覆盖既有建筑。更新 JSON 会改变尚未生成的部分，旧探索边界可能出现接缝；需要调整布局时应在独立新世界检查完整效果。

设计参考：[Minecraft 结构定义](https://minecraft.wiki/w/Structure_definition)、[原版村庄定义](https://raw.githubusercontent.com/misode/mcmeta/data/data/minecraft/worldgen/structure/village_plains.json)、[原版村庄结构集](https://raw.githubusercontent.com/misode/mcmeta/data/data/minecraft/worldgen/structure_set/villages.json)。这里的模板为本项目编写，不包含 Minecraft 模型或贴图。
