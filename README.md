# Remote Monitor (Compliant Edition)

本项目提供两个独立程序：

- `client`：桌面共享客户端（用户可见授权后才会推流）。
- `server`：GUI 服务端（监听并显示实时桌面视频流）。

## 1. 合规目标

1. 明确授权：客户端界面必须勾选授权后才允许开始共享。
2. 可见状态：客户端持续显示当前推流状态，可一键停止。
3. 非静默持久化：开机自启由用户显式勾选后写入 `HKCU\...\Run`。
4. 固定目标地址：服务端地址在代码常量中固定配置。
5. 客户端/服务端解耦：共享协议在公共模块中维护，架构清晰。

## 2. 代码结构

- `src/config.rs`：固定配置（服务端地址、鉴权口令、帧率、帧大小上限）。
- `src/protocol.rs`：TCP 握手与帧封包协议。
- `src/bin/client.rs`：客户端 GUI + 屏幕采集 + JPEG 编码 + 30fps 推流。
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

## 5. Windows 10/11 x64 部署建议

1. 在 Windows 上原生构建 `--release`。
2. 首次运行客户端时，由用户手动确认授权并按需勾选开机自启。
3. 将 `src/config.rs` 中的 `SERVER_TARGET_ADDR`（客户端连接地址）和 `AUTH_TOKEN` 修改为你的实际值后再发布。
4. 服务端默认监听 `SERVER_BIND_ADDR=0.0.0.0:5000`，可按网络策略调整。

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

## 6. 生产环境加固建议（下一步）

当前实现使用口令握手，建议在生产改为：

1. TLS（建议双向证书认证）。
2. 审计日志（连接时间、客户端标识、会话时长）。
3. 多客户端会话管理与权限控制（白名单、会话审批、强制断开）。
