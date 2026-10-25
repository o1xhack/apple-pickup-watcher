# 库存抢东西 · Inventory Rush

Apple 直营店取货库存监控。选门店、选配置，有货通知；App 和手机网页查看同一份数据。

[English](README.en.md) · [下载安装](https://github.com/o1xhack/inventory-rush/releases/latest) · [来源与许可](NOTICE)

![库存统计](assets/screenshots/status-zh.png)

## 安装使用

1. 在 Releases 下载与你的 Mac 对应的压缩包。当前首发包适用于 Apple Silicon（M 系列）；Intel 需要自行构建。
2. 解压，双击「安装并打开.command」。应用会复制到「应用程序」，以后从那里打开。也可以直接把 App 拖进去。
3. 选择地区、输入 ZIP、查找附近门店，选择机型／容量／颜色，点击「添加并开始监控」。
4. 想推送到 iPhone：在运行设置里粘贴你自己的 Bark 推送地址。普通通知默认即可；重要警告须在 iPhone 给 Bark 授权。

本版未公证。若 macOS 拦截，先核对 Releases 提供的 SHA256，再到「系统设置 → 隐私与安全性」允许此 App。安装脚本不会关闭 Gatekeeper，也不会自动抹掉隔离标记。

## App + 网页

点击 App 右上角「网页 / Status」，或在同一台电脑打开 **http://127.0.0.1:17842**。
网页可以添加、编辑监控，查看状态、扫描历史及统计；提供中文和英文，统计页面有截图模式。

手机访问需要电脑保持运行。服务只监听本机；可自行通过 Tailscale Serve 私有共享到同一 tailnet，手机也需连接该 tailnet。本项目不会自动修改 Tailscale、开放公网或生成公用网址。网页没有额外登录页，不要把端口暴露到公网。

![英文统计](assets/screenshots/status-en.png)
![监控界面](assets/screenshots/monitor.png)

## 通知与记录

- 每个目标可选普通或重要通知。默认同一段持续有货只通知一次；打开「有货重复提醒（全部监控）」后，每次新的有货采样都提醒。
- Bark 分组为「Apple 库存监控」；重要提醒使用 anticipate 铃声。
- SQLite 自动保存扫描与通知记录，重启不会丢失。扫描次数不等于手机数量，有货记录也不等于补货次数。
- 查询失败显示未知，HTTP 541 等拦截会自动冷却重试。更短间隔不保证更及时，浏览器查询也不能保证不被拦截。
- 只提供提醒和商品链接，不代下单，不自动加购物袋。

个人设置和历史保存在系统用户数据目录的 `apple-pickup-watcher` 中。**安装包与源码不含你的 Bark 地址、代理、账号、监控清单或数据库。**

## 从源码构建

需要 Rust（最低版本见 Cargo.toml）、Node.js LTS、pnpm 9，以及 [Tauri 系统依赖](https://v2.tauri.app/start/prerequisites/)。

```sh
pnpm install --frozen-lockfile
pnpm tauri build
```

macOS 产物位于 `target/release/bundle/`；App 内已嵌入网页资源，不依赖开发服务器。Windows/Linux 保留 Tauri 构建路径，但本分叉首次发布只提供本机已验证的 macOS Apple Silicon 包；其他平台以构建验证结果为准。安装时把 App 放进应用程序再启动。

## 来源与开源许可

本项目基于 [ENCHIGO/apple-pickup-watcher](https://github.com/ENCHIGO/apple-pickup-watcher)，该项目继承自 [Hteen/apple-store-helper](https://github.com/hteen/apple-store-helper)。

本分叉新增美国 ZIP/门店查询、SQLite 历史与统计、移动网页、中英文网页、逐目标重要提醒、全局重复提醒、浏览器查询及冷却恢复，并修复安装与关窗恢复问题。修改日期与版权详见 [NOTICE](NOTICE)。整体继续以 **GPL-3.0-or-later** 发布，保留上游版权与许可；分发二进制同时提供对应完整源码和构建方式。本项目与 Apple 无关联，不提供库存或抢购成功保证。
