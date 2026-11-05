# 化工设备 Blender 模型

本项目原创模型，沿用仓库 MIT 许可；无第三方模型、纹理或外部下载依赖。使用 Blender **5.2.3 LTS** 制作。

- `chemical_equipment.blend`：可编辑源文件。`Chemical Equipment Library` 场景保存 15 个命名组件；`Equipment Overview` 场景是共享组件网格的展示排布，不是游戏世界布局。
- `../../godot/assets/industry/chemical_equipment.glb`：游戏使用的 glTF 2.0 二进制资源，含网格和 PBR 材质。Godot 导入不需要安装 Blender。
- `../../godot/src/industry_models.gd`：按 Rust 提供的尺寸、端口和方向拼装，按材质合并为一个设备网格；`industry_view.gd` 按形状复用资源，运行读数变化不重新拼装。预览和实际放置共用模型，退出会话清理缓存。

覆盖常压罐、密闭罐、直管/两口弯管、泵、阀、双流换热器、单管架、三口蒸馏塔、五口回流蒸馏塔、冷凝器、再沸器和三通。设备的机械细节与仪表刻面仅用于外观，不代表新增过程功能或实时仪表读数。

## 编辑与导出

1. 在 Blender 打开源文件，切换到 `Chemical Equipment Library` 场景。保留组件名称、原点、材质和单位；不要导出展示场景或默认启动场景。
2. 修改后全选该库场景的网格，导出 glTF 2.0，格式选择 **GLB**，启用 **Selected Objects / Apply Modifiers / +Y Up**，导出到上述游戏资源路径。模型已三角化；硬边已拆分，圆柱侧面和封头保留平滑法线。不要导出相机、灯光或动画。
3. 保留 `.glb.import` 的稳定 UID，运行 `just godot-industry-models-graphics`，再运行受影响设备的现有图形验收。更新下面的 SHA-256。

源文件使用 Blender Z 向上；下表坐标均指导出后的 Godot Y 向上坐标，1 单位 = 1 米。固定设备组件位于其占用格的局部坐标；缩放组件原点在底面轴心，接口组件原点在接口中心。

| 组件 | 坐标约定与用途 |
| --- | --- |
| `open_shell` | 半径 1、高度 1，底部封闭、顶部开口；缩放成罐/塔筒体 |
| `dished_head` | 半径 1、高度 1 的碟形封头，向 +Y 凸起 |
| `shell_band` | 外半径 1、高度 1 的加强环 |
| `plinth` | 半径 1、高度 1 的底座/密闭罐顶部人孔 |
| `ladder_metre` | 1 米梯段，朝 −Z，梯宽 0.365 米；沿高度重复 |
| `gauge` | 朝 −Z 的静态表盘；不显示模拟读数 |
| `pipe_shaft` | 从原点沿 +X 长 1 米，半径 0.087 米 |
| `port_flange` | 从原点沿 +X 伸入设备，外半径 0.12 米，8 个六角螺栓 |
| `pipe_joint` | 原点处半径 0.108 米的紧凑接头 |
| `pump` / `valve` | 1 格设备，中心 `(0.5,0.5,0.5)`，主管沿 X；整体旋转支持三轴 |
| `pipe_rack` | 单格支架，顶面低于 Y=0.375；仅按现有 X/Z 姿态旋转 |
| `heat_exchanger` | 2×1×3 格板式换热器主体，橙/蓝两条独立流道 |
| `condenser` / `reboiler` | 3×2×3 格卧式壳体、端盖、鞍座和内部连接外观 |

法兰和外部短管由 Rust 返回的端口放置；Godot 不自行计算进料口、三通接头或权威碰撞。可变罐径/塔高仅缩放壳体、封头和环，法兰、螺栓、梯段保持独立尺寸。梯子/表盘收在已有碰撞包络内，因此没有新增可攀爬规则。开口罐内部仍保留原来的实心碰撞与单液模型。

## 验证

`just godot-industry-models` 检查 **908** 种组合：全部 2–6 米罐径及 3–12 米罐高，两类塔的全部 2–12 板及每一个进料板位置，15 组两口管接头，12 个三通姿态，泵/阀三轴与两个管架方向，以及固定换热设备。检查实际导入网格所有顶点落在 Rust 共享碰撞盒联合内（1 毫米导入浮点容差）、端口端点、材质和无效规格拒绝。图形总览单独运行，避免穷举网格上传到 GPU。

`just godot-industry-models-graphics` 先执行上述检查，再用 Forward+ / SDFGI 渲染 `target/blender-models-gallery.png`。这张图是模型总览；真实世界中的建造、同步、控制和保存由现有 `godot-industry-*` / `godot-column-loop-graphics` 验收覆盖。没有新增神经生成或运行时 Blender 依赖。

| 文件 | SHA-256 |
| --- | --- |
| `chemical_equipment.blend` | `b9212af67d149db10ae7532ed8fb78b1b3c4233de264166ea59bbe2653a568e6` |
| `chemical_equipment.glb` | `29fd85dc7b1ae17af787bfbe98db87b6ab82006e8f711bbbd11b8dfd8f73bce7` |
