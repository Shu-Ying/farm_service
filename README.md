# farm_service

真寻文字农场的 Rust 服务端起点。当前实现用户基础资料上报和查询，
另提供可选的公开素材分发接口，尚未实现活动发布中心，
也尚未接入 QQ 机器人自动上报。

## 当前能力

- `GET /health`：公开就绪检查，实际检查数据库。
- `GET /v1/assets/manifest.json`：公开素材清单，含版本、SHA256、大小及三源绝对URL。
- `GET /v1/assets/{sha256}/assets.zip`：下载启动时校验过的素材快照，支持ETag。
- [资源分发接口与三端接入约定](docs/assets.md)。
- 素材发布文件位于根目录 `assets/manifest.json` 和 `assets/assets.zip`；
  ZIP 仅由本服务仓库发布，三个插件分发包不包含 ZIP。
- `PUT /v1/users`：受保护的用户资料新增/更新；重复上报不会重复建档。
- `GET /v1/users/{platform}/{platform_user_id}`：受保护的单用户查询。
- 独立 SQLite 数据库，启动自动迁移；不访问农场插件的数据库。
- 每日 JSON 文件日志；不记录 QQ 号、昵称、令牌、完整请求或 SQL 参数。
- AGPL v3 开源协议（`AGPL-3.0-only`）。二进制应用的 `Cargo.lock` 应纳入版本控制。

此阶段用 SQLite 降低首次运行门槛。此前活动规划中的 PostgreSQL、
发布目录、活动账户和跨库奖励交付尚未实施，不创建占位字段或空模块。

## 用户字段与身份

| 字段 | 用途与约束 |
|---|---|
| `platform` | 平台命名空间：`qq`、`telegram`、`discord` |
| `platform_user_id` | 字符串形式的正整数 ID，不接受前导零；QQ 最多 20 位，其他最多 32 位 |
| `nickname` | 当前昵称，去除首尾空白后 1～64 个 Unicode 字符，禁止控制字符 |
| `created_at` | 首次建档 UTC 时间，更新昵称时保留 |
| `updated_at` | 最近资料写入 UTC 时间 |

`platform + platform_user_id` 是唯一键。同一 QQ 在不同机器人上报时
对应同一份基础资料，不包含农场余额、机器人内玩家归属或群成员信息。
这些资料来自持有令牌的受信机器人，不代表服务端已向 QQ 验证身份。

`qq` 只表示普通 QQ 号码（如 OneBot 适配器提供的标识），
不接受官方 QQ OpenID/UnionID，也不把它们自动转换成 QQ 号。
以后接入官方 QQ 时须增加应用作用域，避免不同应用的 OpenID 被混用。
不要另外添加与 `platform_user_id` 重复、无独立用途的 `qq_number` 字段。

## 启动

需要 stable Rust 工具链和本机 C/C++ 构建工具（SQLite 原生依赖需要）。
Windows 推荐 Visual Studio Build Tools 的“使用 C++ 的桌面开发”。
依赖版本由 `Cargo.lock` 固定；首次构建需要访问 crates.io。

PowerShell 示例：

```powershell
# 仅供本地启动。生成令牌后妥善保存，调用方使用同一令牌。
$env:FARM_API_TOKEN = [Convert]::ToHexString(
  [System.Security.Cryptography.RandomNumberGenerator]::GetBytes(32)
)
$env:FARM_BIND = "127.0.0.1:8080"
cargo run --locked
```

| 环境变量 | 默认值 / 作用 |
|---|---|
| `FARM_API_TOKEN` | 必填，32～256 个可见 ASCII 字符；建议随机生成 |
| `FARM_BIND` | `127.0.0.1:8080` |
| `FARM_DATA_DIR` | `./data`，保存 `farm_service.sqlite3` |
| `FARM_LOG_DIR` | `./logs`，保存每日滚动日志 |
| `RUST_LOG` | `farm_service=info` |
| `FARM_ASSET_MANIFEST` | 可选，建议 `./assets/manifest.json`；同目录须有assets.zip；未设置时资源接口返回503 |

相对目录以进程工作目录为基准；生产建议使用固定绝对路径。
`.env.example` 只是模板，程序**不会自动加载 .env**。
Ctrl+C 停止；Unix 同时支持 SIGTERM，等待在途 HTTP 请求结束后关闭连接池。
日志按日切分，不自动清理历史日志；部署时配置保留期限和磁盘监控。

插件的「资源获取地址」与「farm_service 地址」始终同时保留。
资源源可选 `Github`、`Gitea`、`farm_service`，默认 `Github`；
服务地址独立供资源、活动等功能使用，活动接口当前尚未实现。
GitHub/Gitea 的清单及 ZIP 均从本服务仓库根目录 `assets/` 的 raw 直链读取，
详见[资源发布位置](docs/assets.md#仓库文件与发布位置)。
当前本地清单中的 `https://farm.example` 明确为占位地址，
客户端选择服务源时使用其独立配置的真实基础地址。
本轮仅准备本地文件，未提交、推送或部署，远端可用性尚未验证。

## 调用示例

在启动服务时设置过令牌的 PowerShell 会话中执行：

```powershell
$headers = @{ Authorization = "Bearer $env:FARM_API_TOKEN" }
$profile = @{
  platform = "qq"
  platform_user_id = "12345678"
  nickname = "农场玩家"
} | ConvertTo-Json

Invoke-RestMethod -Method Put -Uri "http://127.0.0.1:8080/v1/users" `
  -Headers $headers -ContentType "application/json; charset=utf-8" `
  -Body ([System.Text.Encoding]::UTF8.GetBytes($profile))

Invoke-RestMethod -Uri "http://127.0.0.1:8080/v1/users/qq/12345678" `
  -Headers $headers
```

写入和更新均返回 HTTP 200 与完整资料。JSON 只接受上述三个输入字段。
请求体上限 4 KiB。非法资料返回 400；JSON 类型/格式错误可能返回
400/415/422；超大请求返回 413；鉴权失败返回 401；用户不存在返回 404；
存储不可用返回 503，不向客户端泄漏数据库细节。

## 接入与部署边界

插件需要在接入层从可信事件取得用户 ID、昵称及平台，再显式上报；
当前不会自动读取 QQ、扫描群成员或收集历史消息。
只在确有业务用途时上报这些字段，并向用户说明用途及保留策略。

本版本一个令牌对应一个受信服务访问范围，持有者可以查询或更新已知 ID
的资料，尚无租户级权限或管理员账户。令牌不能放进群消息、前端或日志。
暂不提供用户枚举/批量查询接口。

默认仅监听本机；公网部署需 HTTPS 反向代理、限流及访问控制。
反向代理的 URL 访问日志可能包含查询路径中的用户 ID，应关闭该路径的
访问日志或脱敏。数据目录/备份仅授予服务账户权限，备份按个人资料保护。
用户删除和保留政策需在正式收集前落实，本轮未实现删除接口或自动清理。

## 开发验证

```text
cargo fmt --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
```

测试覆盖鉴权、资料校验、昵称更新、重启持久化、平台隔离、并发建档、
请求大小及数据库不可用。测试使用临时目录，不写入实际用户数据库。

## 开源协议

Copyright (c) 2026 Shu-Ying。
本项目采用 GNU Affero General Public License v3.0，
SPDX 标识为 `AGPL-3.0-only`，完整条款见 [LICENSE](LICENSE)。

允许使用、修改、分发及商用，但须遵守协议的版权、源码提供等要求。
特别是修改后的版本通过网络与用户交互时，应按第 13 节向这些用户
显著提供免费取得该修改版本对应源码的机会。
源码地址应指向实际运行版本，不能仅指向不包含这些修改的上游仓库。
协议针对程序及其对应源码，不要求公开 API 令牌、用户资料或数据库内容。
