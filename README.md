# Remote Monitor (Compliant Edition)

本项目提供两个独立程序：

- `client`：Windows 优化客户端（无控制台 GUI，服务端地址硬编码，注册表 `Run` 自启动）。
- `server`：GUI 服务端（监听并显示实时桌面视频流）。

## 1. 合规目标

1. 明确授权：客户端 GUI 必须勾选授权后才允许开始共享。
2. 可见状态：客户端 GUI 持续显示连接与推流状态，可一键停止。
3. 非静默持久化：开机自启由用户显式勾选后写入注册表 `HKCU\...\Run`。
4. 固定目标地址：服务端地址在代码常量中固定配置。
5. 客户端/服务端解耦：共享协议在公共模块中维护，架构清晰。

## 2. 代码结构

- `src/config.rs`：固定配置（服务端地址、鉴权口令、帧率、帧大小上限）。
- `src/protocol.rs`：TCP 握手与帧封包协议。
- `src/bin/client.rs`：Windows GUI 客户端 + Windows Graphics Capture 截屏 + JPEG 编码 + 30fps 推流 + 注册表自启动。
- `src/bin/server.rs`：服务端 GUI + 监听 + 解码渲染。

## 3. 通信协议（简化）

1. 客户端连接后发送握手：
   - `magic(u32)` + `version(u16)`；
   - `token_len(u16)+token`；
   - `client_name_len(u16)+client_name`。
2. 服务端校验通过后接收帧：
   - `frame_len(u32)` + `jpeg_bytes`。

## 4. 运行方式

```bash
cargo run --bin server
```

另开一个终端：

```bash
cargo run --bin client
```

客户端服务端地址在 `src/bin/client.rs` 中硬编码：

```rust
const SERVER_ADDR: &str = "127.0.0.1:5000";
```

## 5. Windows 10/11 x64 部署建议

1. 在 Windows 上原生构建 `--release`。
2. 首次运行客户端时，用户可在 GUI 中勾选“开机自动启动客户端（当前用户，注册表 Run）”。
3. 将客户端 `src/bin/client.rs` 中的 `SERVER_ADDR`、以及 `src/config.rs` 中的 `AUTH_TOKEN` 修改为你的实际值后再发布。
4. 服务端默认监听 `SERVER_BIND_ADDR=0.0.0.0:5000`，可按网络策略调整。
5. 客户端使用 `#![windows_subsystem = "windows"]`，双击启动不弹控制台。
6. 客户端截图引擎为 Windows Graphics Capture（适配 Windows 10/11，建议在用户会话中运行）。

## 5.1 服务端跨平台保证（Linux/Windows）

1. 服务端代码不依赖 Windows 专有 API。
2. 仓库内置 CI：`.github/workflows/server-cross-platform.yml`，在 `ubuntu-latest` 与 `windows-latest` 上执行 `cargo check --bin server`。
3. Linux 桌面环境下运行 GUI 需要可用显示服务（X11 或 Wayland）。
4. 若 GUI 中文显示方块，请安装中文字体（如 `fonts-noto-cjk` 或 `wqy-zenhei`）。

示例（Debian/Ubuntu）：

```bash
sudo apt-get update
sudo apt-get install -y fonts-noto-cjk
```
