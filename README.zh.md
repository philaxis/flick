<p align="center"><a href="README.md">English</a> · <a href="README.ko.md">한국어</a> · <a href="README.zh.md">中文</a></p>

<p align="center"><img src="assets/icon-256.png" width="96" alt="Flick"></p>

<p align="center"><strong><a href="https://github.com/philaxis/flick/releases/latest/download/Flick.exe">⬇ 下载 Windows 版 Flick</a></strong></p>

<p align="center">按住按键,轻推鼠标,就到了旁边的桌面。</p>

<p align="center"><a href="https://philaxis.github.io/flick/">在 Flick 网站上看它怎么用</a></p>

---

**桌面变成网格。** 左右是同一项工作的几个屏幕,上下是其他工作。

**按住再推。** 按住触发键,把鼠标往一个方向推,就往那边移动一格。指针留在原地。

**轻点看全部。** 轻点触发键,会显示当前桌面的窗口和一张小地图。把窗口拖到地图的格子上就能移过去。

**按键随你定。** 默认是鼠标的“前进”侧键。右键点击托盘图标 → *更改触发键*,按下你想用的按键即可,也可以同时按几个键。

**小巧免费。** 一个不到 1 MB 的 exe。不用注册,没有安装向导。开源,MIT 许可。

## 开始使用

1. 用上面的按钮下载并运行。它会自行安装,并随 Windows 启动。
2. 按住触发键,把鼠标往旁边推。
3. 轻点触发键,在地图上按 **+** 添加桌面或新的一行。

## 须知

- 支持 Windows 11 23H2(内部版本 22631.3085 及以上)、24H2(内部版本 26100.2605 及以上)和 25H2。在其他版本上只会提示,不会启动。
- 卸载:设置 → 应用 → 已安装的应用 → Flick。

## 构建

在 Windows 上运行 `cargo build --release`,在 WSL 中运行 `scripts/build-wsl.sh build --release`。

[MIT](LICENSE)。`vendor/winvd` 和 `vendor/winvd-24h2` 来自 [VirtualDesktopAccessor](https://github.com/Ciantic/VirtualDesktopAccessor)(MIT)。
