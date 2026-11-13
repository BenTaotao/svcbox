# svcbox

Windows 本地开发环境 Web 服务管理器：在一个界面里管理多套隔离的 **Nginx / MySQL / Redis** 实例，作为日常开发的本地工具。

- 每个服务实例独立目录、独立端口、独立配置，互不干扰
- Windows 用 **JobObject** 托管进程组，停止时一次性销毁子进程与孙进程，不留僵尸进程
- 安装包全部来自官方地址，下载后 **强制 sha256 校验**，原始压缩包留在缓存里复用
- 所有配置修改走 **备份 → 模板渲染 → 语法校验 → 失败自动回滚** 的闭环
- GUI（Tauri 2 + React 19）为主，CLI 能力对齐，可选本机 HTTP API
- 从 GUI 启动服务时使用 `CREATE_NO_WINDOW` 隐藏子进程控制台，不会弹黑色命令行窗口

## 技术栈

| 层 | 选型 |
| --- | --- |
| 桌面 | Tauri 2 |
| 前端 | React 19 + TypeScript + Vite |
| 内核 | Rust + tokio |
| 持久化 | rusqlite（bundled SQLite） |
| 模板 | handlebars |
| 进程托管 | Windows JobObject / Unix 进程组 |
| 下载 | reqwest + sha2，断点续传 |
| 证书 | acme.sh（Let's Encrypt，HTTP-01） |
| CLI | clap derive |
| HTTP API | axum（可选，仅监听 127.0.0.1） |

## 目录

```
.
├── src/                     # Rust 内核 + CLI + Tauri 命令
│   ├── core/                # 业务内核，不依赖 Tauri
│   │   ├── db.rs            # SQLite
│   │   ├── download.rs      # 下载 / 断点续传 / sha256 / 解压
│   │   ├── version.rs       # 版本清单与安装
│   │   ├── instance.rs      # 实例生命周期
│   │   ├── process.rs       # JobObject / 进程组
│   │   ├── health.rs        # 三重健康检测
│   │   ├── config.rs        # 备份 / 渲染 / 校验 / 回滚
│   │   ├── cert.rs          # acme.sh 证书申请与续期
│   │   └── service/         # nginx / mysql / redis 能力
│   ├── templates/           # handlebars 模板
│   ├── api.rs               # 本地 HTTP API
│   ├── cli.rs               # svcbox cli 子命令
│   ├── commands.rs          # Tauri invoke 包装
│   └── main.rs
├── ui/                      # React 前端
├── icons/
├── manifest.json            # 内置版本清单（官方下载地址 + sha256）
├── tauri.conf.json
├── capabilities/
└── Cargo.toml
```

所有下载文件、版本、实例数据、数据库、证书、日志默认都在 **`D:\BC\svcbox`**（没有 D 盘或目录不可写时回退到 `%APPDATA%\svcbox`）。可在设置页修改，或启动时用 `--home` / `--data-dir` 指定任意可写目录。

```
D:\BC\svcbox\
├── svcbox.db
├── secret.key              # 本机凭据加密密钥
├── versions/{kind}/{version}/
├── instances/{name}/
│   ├── conf/               # 主配置 + 站点 + backup
│   ├── logs/               # 服务日志 + audit.log
│   ├── run/                # pid / socket
│   └── ...
├── cache/packages/         # 官方安装包缓存
├── cache/acme/
├── templates/              # 自定义模板（覆盖内置同名文件）
└── logs/
```

设置页修改数据目录后，只在 `%APPDATA%\svcbox\bootstrap.json` 留一个很小的指针文件；实际业务数据全部在新目录，不会自动搬迁旧数据。

## 环境要求

- Windows 10/11 x64（Linux 下进程组逻辑已实现，未做发行验证）
- Rust 1.80+、Node.js 18+
- **GUI 构建**：Visual Studio Build Tools（C++ 工作负载）+ WebView2 Runtime
- **HTTPS 证书**：Git for Windows（提供 bash / curl / openssl）；域名需解析到本机且放行 80 端口

## 构建与运行

```bash
npm install

# 前端生产构建
npm run build

# CLI / HTTP API（不依赖 Tauri，任何平台可编译）
cargo build --release
# 产物: target/release/svcbox(.exe)

# 桌面 GUI（需 MSVC + WebView2；custom-protocol 让前端 dist 嵌入二进制，不再依赖 Vite）
cargo build --release --features desktop,custom-protocol
```

开发模式：

```bash
# 终端 1
npm run dev
# 终端 2
cargo run --features desktop
```

> macOS / Linux 下 desktop feature 也可尝试构建，但 Tauri 打包与 WebView 依赖需要按平台安装额外系统包。

## CLI 速查

```bash
# 版本
svcbox cli version list
svcbox cli version install nginx 1.30.6
svcbox cli version remove nginx 1.30.6
svcbox cli version prune          # 清理不在当前清单、且没有实例引用的旧版本
svcbox cli version import mysql 8.4.13 D:/downloads/mysql-8.4.13-winx64.zip --sha256 <hash>
svcbox cli version refresh --url https://example.com/svcbox-manifest.json

# 实例
svcbox cli instance create nginx dev --version 1.30.6 --port 8080
svcbox cli instance create mysql db --version 8.4.13 --port 3306
svcbox cli instance start dev
svcbox cli instance stop dev
svcbox cli instance restart dev
svcbox cli instance list
svcbox cli instance remove dev          # 默认删除实例目录，--keep-data 可保留
svcbox cli logs dev -f
svcbox cli doctor

# Nginx
svcbox cli nginx site create dev test.com --root D:/www/test
svcbox cli nginx site list dev
svcbox cli nginx site delete dev test.com
svcbox cli nginx reload dev
svcbox cli nginx https apply dev test.com
svcbox cli nginx cert renew dev test.com

# MySQL（实例必须处于运行状态）
svcbox cli mysql db create dev testdb
svcbox cli mysql db list dev
svcbox cli mysql db drop dev testdb
svcbox cli mysql user create dev testuser localhost 123456 --db testdb
svcbox cli mysql user list dev
svcbox cli mysql user pass dev testuser localhost 654321
svcbox cli mysql user drop dev testuser localhost
svcbox cli mysql root-pass dev 123456

# Redis（bind / port / 密码 / AOF 修改后需重启）
svcbox cli redis config set dev --bind 127.0.0.1 --port 6380 --pass 123456 --aof true
svcbox cli redis config show dev

# 本地 HTTP API
svcbox cli api serve --port 9090
```

所有命令都支持 `--home <目录>` 指定数据目录，默认 `D:\BC\svcbox`（无 D 盘或不可写时回退 `%APPDATA%\svcbox`）。GUI 也支持 `--home` / `--data-dir`，设置页修改后需重启生效。

`instance start` 会拉起一个脱离终端的后台守护进程（`__supervise` 内部子命令），由它持有 JobObject。因此 CLI 退出、关闭终端后服务仍然运行；`stop` 会请求守护进程优雅停机。CLI 守护进程托管的实例也能在 GUI 中看到运行状态并停止；如果实例由另一个 GUI 进程托管，CLI 会拒绝跨进程停止并提示改用 GUI 或 HTTP API。

## HTTP API

设置页开启后监听 `127.0.0.1:9090`（可改端口），仅本机、无鉴权；服务端校验 Host 必须为本机、Origin 必须为本机或 Tauri，阻止 DNS rebinding。主要路由：

```
GET    /api/health
GET    /api/instances
POST   /api/instances
GET    /api/instances/:id
POST   /api/instances/:id/start|stop|restart
DELETE /api/instances/:id?deleteData=true
GET    /api/instances/:id/logs?lines=200
GET    /api/versions
POST   /api/versions/install
GET    /api/instances/:id/nginx/sites
POST   /api/instances/:id/nginx/sites
POST   /api/instances/:id/nginx/https
GET    /api/instances/:id/mysql/dbs
GET    /api/instances/:id/mysql/users
PUT    /api/instances/:id/redis
GET    /api/certs
GET    /api/settings
GET    /api/doctor
```

错误统一返回 `{"error": "..."}`，状态码区分 400 / 404 / 409 / 500。

## 安全设计

1. **进程组隔离**：Windows 创建带 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 JobObject，进程退出时整组销毁；Linux 用 `setpgid` + 进程组信号。CLI 启动的实例由独立守护进程持有 JobObject，终端退出不影响服务。
2. **完整性校验**：清单中 sha256 为空时拒绝安装；下载或手动导入都会校验，失败删除缓存并报错。
3. **凭据加密**：MySQL root 密码、Redis 密码用 AES-256-GCM 加密后写入 SQLite，密钥保存在数据目录 `secret.key`。
4. **配置回滚**：写入前备份（保留最近 30 份），Nginx 用 `nginx -t` 校验，失败恢复备份并把 stderr 原样返回。
5. **SQL 安全**：不开放任意 SQL，库名/账号名/密码全部校验后参数化拼接；账号只授予 `SELECT,INSERT,UPDATE,DELETE,CREATE,DROP,ALTER,INDEX`，不开放 `SUPER/FILE/REPLICATION`。
6. **审计**：账号、库、证书、配置变更写入实例 `logs/audit.log`，密码字段自动脱敏。
7. **端口前置检测**：建实例、改端口、启动前都先尝试 bind，避免启动后才失败。
8. **HTTP API 防跨站**：只监听 127.0.0.1，并校验 Host 必须是本机、Origin 必须是本机或 Tauri，阻断 DNS rebinding 与浏览器跨站调用。
9. **Redis 优雅停机**：直接通过 RESP 发送 AUTH/SHUTDOWN，密码不进入命令行；停机等待 15 秒，避免打断 RDB/AOF 落盘。

## 版本清单

内置 `manifest.json` 固定三个包，sha256 由官方安装包实际计算：

| 类型 | 版本 | 来源 |
| --- | --- | --- |
| Nginx | 1.30.6 | nginx.org（最新稳定版） |
| MySQL | 8.4.13 | cdn.mysql.com 官方 CDN（8.4 LTS 最新版） |
| Redis | 8.10.3 | redis-windows/redis-windows（Windows 社区构建） |

可配置远端清单（HTTPS JSON 数组，字段与内置一致），远端同 `kind+version+platform` 覆盖内置。`mirror_url` 用于「国内镜像备用源」开关。

说明：

- MySQL 选 8.4 LTS 最新补丁（不是 9.x Innovation 版本）。下载走 `cdn.mysql.com` 官方 CDN 直链；`dev.mysql.com` 的下载入口会按 User-Agent 拦截非浏览器客户端（返回 403）。国内公共镜像目前没有同步 8.4 的 Windows 包，选择“国内镜像”时会自动回退官方 CDN，仍可正常下载。
- Redis 官方不发布 Windows 原生包，内置清单使用 `redis-windows/redis-windows` 的 MSYS2 构建，压缩包自带运行库，启动时使用实例目录下的相对配置路径以兼容其路径解析。

## 测试

```bash
cargo test
```

覆盖了实例名校验、域名/库名校验、审计脱敏、配置模板渲染、配置回滚、Redis 配置校验、zip 路径穿越防护、SQL 转义、加密往返、进程退出检测等纯逻辑。

## 常见问题

### MySQL 报 `1130 - Host '127.0.0.1' is not allowed to connect`

旧版本生成的 `my.ini` 带 `skip-name-resolve`，MySQL 不会把 TCP 连接来源 `127.0.0.1` 解析成 `localhost`，因此 `root@localhost` 和 `用户@localhost` 都会拒绝连接。

新版本已移除该配置；旧实例在 GUI 启动或下次启动时会自动清理这一行，**重启该 MySQL 实例后生效**。之后用 `127.0.0.1` 连接 `@localhost` 账号即可。若仍失败，请检查账号 host 是否为 `%` 或 `127.0.0.1`。

## 已知限制

- **GUI 构建工具链**：官方推荐 MSVC（VS Build Tools + WebView2）。本机也用 WinLibs + `stable-x86_64-pc-windows-gnu` 成功编译、链接并启动过桌面端（仅有 `multiple non-default manifests` 链接器警告）；生成 MSI 安装包仍建议使用 MSVC 工具链。
- **acme.sh 需要 bash**：Windows 上依赖 Git for Windows。HTTP-01 要求实例监听 80 端口、域名已解析到本机、防火墙放行。
- **跨进程停止有边界**：CLI 可以停止自己拉起的守护进程；GUI 也能停止 CLI 守护进程托管的实例。但 CLI 无法直接操作另一个 GUI 进程内存里的进程表，遇到这种情况会返回明确提示，请改用 GUI 或 HTTP API。
- **`api serve` 是常驻进程**：它持有自己启动实例的 JobObject，退出该进程会停止由它启动的服务。GUI 内置的 HTTP API 跟随 GUI 生命周期。
- 证书一期仅支持 HTTP-01，不做 DNS-01 与内网自签。
- MySQL 密码重置会短暂以 `--skip-grant-tables` 运行，期间数据库不对外提供认证服务，属于高风险操作，UI 有二次确认。
- Nginx 的 HTTPS 端口默认 8443（多实例不能共用 443），可在实例「站点与 HTTPS」页签修改。

## 许可

MIT
