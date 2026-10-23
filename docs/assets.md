# 资源分发接口与三端接入约定

本文件位于 farm_service 项目。服务端接口及准备工具已实现，
素材发布文件位于本仓库根目录 `assets/`。当前文件只完成本地准备，
尚未提交或发布到 GitHub/Gitea，不能据此认定远端直链已经可用。
活动接口尚未实施，不因为预留服务地址就返回虚构的活动数据。

## 已确认的客户端配置约定

两个配置项始终同时存在，不根据选项隐藏：

| 配置 | 约定 |
| --- | --- |
| 资源获取地址 | 选项 Github、Gitea、farm_service；默认 Github |
| farm_service 地址 | 独立的服务基础地址，供资源、活动等功能共同使用 |

选择 Github 或 Gitea 时，farm_service 地址仍保留，未来活动查询仍可使用。
选择 farm_service 获取资源时，该地址必须有效。客户端根据此配置构造
`/v1/assets/{sha256}/assets.zip` 请求，不依赖清单中服务地址的占位值。
三端接入状态及具体配置字段以插件实现为准，不能把此文档当作线上验证。

Farm当前本地已接入共享启动校验/下载器，三端插件包不再包含assets.zip。
NoneBot字段为`FARM_RESOURCE_SOURCE`、`FARM_SERVICE_URL`；AstrBot字段为
`resource_source`、`farm_service_url`；真寻配置名为「资源获取地址」、
「farm_service 地址」。均默认Github，服务地址默认空，修改后重载。
源码及离线测试已适配不代表真实公网下载或宿主验证已完成。

## 仓库文件与发布位置

```text
assets/
  manifest.json
  assets.zip
```

`assets/manifest.json` 和 `assets/assets.zip` 是实际发布文件；
`docs/assets.manifest.example.json` 与实际清单保持一致，仅用于阅读示例。
ZIP 从现有农场素材复制，源文件不由准备工具删除。
后续素材发布只更新并提交 farm_service 仓库内的这组文件，
不要把 ZIP 加回 NoneBot、真寻或 AstrBot 的插件仓库、生成目录或发布包。
插件可以携带绑定版本的轻量清单，但不携带 `assets.zip`。

发布到 `main` 分支后，公开静态清单直链为：

- GitHub：`https://raw.githubusercontent.com/Shu-Ying/farm_service/main/assets/manifest.json`
- Gitea：`https://gitea.diuse.work/Diuse/farm_service/raw/branch/main/assets/manifest.json`

对应 ZIP 位于相同目录，文件名为 `assets.zip`。选择 GitHub/Gitea 时
可独立访问公开静态清单和 ZIP，无需 farm_service 在线。
本轮没有提交、推送或验证这些远端文件。

## 服务端部署文件

服务不从用户输入下载ZIP，也不代理任意URL。管理员准备本地发布文件；
`FARM_ASSET_MANIFEST`指向JSON，其同目录固定文件名为`assets.zip`。
启动读取并检查schema、版本、URL、大小、整个ZIP的SHA256及ZIP文件头。
校验失败拒绝启动，不对外提供损坏素材。未设置该环境变量时，其余服务
正常启动，资源接口返回503。现有用户资料接口仍需要Bearer鉴权。

ZIP不解压。启用后保留最多64 MiB的单份只读内存快照，请求共享这份内容；
不会每个请求重复读取磁盘和计算SHA。运行中修改磁盘不会改变当前返回内容，
更新清单或ZIP需先准备一套完整目录，再修改配置并重启服务。
当前仅托管一个活动资源版本；请求其他SHA返回404，历史版本仍需保留在
GitHub/Gitea中，或以后扩展多版本服务。资源版本不必等于插件版本。

## 用Rust工具生成清单

不手填SHA。工具从现有ZIP计算SHA256与字节数，并复制文件、输出JSON；
输出目录必须不存在，不覆盖旧发布目录，不上传、不访问远端。

[实际清单](../assets/manifest.json)及[完整JSON示例](assets.manifest.example.json)
已使用当前农场 ZIP 生成真实 SHA 与大小。生成根目录 `assets/` 的示例如下；
已存在时请选择新的临时输出目录，检查后再替换一整套发布文件。

```powershell
cargo run --locked --bin prepare_assets -- `
  D:\Work\farm\src\farm_render\data\assets.zip `
  .\assets `
  farm-assets-v1 `
  https://raw.githubusercontent.com/Shu-Ying/farm_service/main/assets/assets.zip `
  https://gitea.diuse.work/Diuse/farm_service/raw/branch/main/assets/assets.zip `
  https://farm.example
```

`https://farm.example` 是明确的占位域名（placeholder），不是已部署服务。
目前未提供公网地址，所以实际清单及示例暂保留该值；客户端使用独立的
「farm_service 地址」配置构造请求，不请求该占位地址。
部署时应通过准备工具改为真实公网基础地址，支持反向代理路径前缀。
公网访问由部署者配置HTTPS代理，
服务仍默认监听127.0.0.1:8080。不要使用GitHub/Gitea仓库网页URL替代raw文件URL。

准备完成后设置环境变量（PowerShell示例）：

```powershell
$env:FARM_ASSET_MANIFEST = (Resolve-Path .\assets\manifest.json).Path
# FARM_API_TOKEN等原有必填配置沿用原部署方式，不在本文放入真实令牌。
cargo run --locked --bin farm_service
```

此时依旧需要原有用户服务的FARM_API_TOKEN；公开资源下载不使用它。
不要将资料API令牌打包进插件或写入资源JSON。

## JSON协议

`schema`固定为1；工具实际生成有效64位小写SHA256，下面仅解释字段：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| schema | 整数 | 清单协议版本1 |
| version | 字符串 | 资源发布标签，1～64个字母、数字、点、下划线或连字符 |
| sha256 | 字符串 | 整个ZIP的64位小写SHA256 |
| size_bytes | 整数 | ZIP字节数，1～67,108,864 |
| sources.github | 字符串 | GitHub绝对HTTPS下载URL |
| sources.gitea | 字符串 | Gitea绝对HTTPS下载URL |
| sources.farm_service | 字符串 | 服务绝对HTTPS下载URL，包含相同SHA256 |

JSON不接受未知字段；URL不允许凭据、查询参数或fragment。
清单大小限制16 KiB。服务端校验本地ZIP，不会主动探测两个镜像是否一致；
发布者须确保三源文件字节相同。主分支raw文件可能变更，长期建议采用
固定提交、标签或Release附件直链。SHA固定而raw发生变化时，客户端必须
拒绝新字节，不能通过重新获取任意清单来跳过版本兼容性检查。

## HTTP接口

### GET /v1/assets/manifest.json

公开，无需资料API令牌；200返回上述JSON，Cache-Control为no-cache。
未启用资源服务时503，错误码`assets_unavailable`。

### GET /v1/assets/{sha256}/assets.zip

公开，只有SHA与当前校验资源一致时返回200，Content-Type为application/zip。
下载内容与清单的SHA及大小一致；ETag为带双引号的完整SHA。
支持If-None-Match条件请求，命中返回304；返回不可变长期缓存标记。
未知或旧SHA返回404及`asset_not_found`。不支持任意路径访问，也不执行解压。
不是活动接口，不读取玩家库。未启用资源服务时返回503。

## 三端下载器接入约定

1. 启动时计算本地ZIP的SHA256；符合插件绑定的资源版本和摘要则直接复用。
2. 缺失或不匹配才按「资源获取地址」选择源下载；网络不可用不应损坏旧文件。
3. 将内容下载到临时文件，限制字节数和超时，校验大小、SHA及必要ZIP条目，
   通过后原子替换，避免并发启动重复下载。
4. 只有摘要和布局元数据匹配的ZIP才能参与绘图，不能任意跟随服务的最新版本。
5. 清单可以从farm_service接口读取，也可将工具生成的同一JSON发布到镜像，
   并随插件保留经过版本绑定的清单，避免选择Github后又强依赖服务在线。
6. 三端共用同一实现；无可用素材时保留文字功能并提示管理员，不能使用损坏包。
7. 当前按所选源下载，不自动跨源回退；失败保留文字模式，调整配置后重载重试。
8. 本地缓存路径为插件数据目录`assets/<sha256>/assets.zip`，可手动预置离线包。
   同进程并发启动合并下载，跨进程使用独立临时文件与原子替换保证写入完整。
   不解压到磁盘，缓存中存在旧版本时不自动删除，便于回滚。

## 离线验证

```text
cargo fmt --check
cargo test --locked --offline --all-targets
cargo clippy --locked --offline --all-targets -- -D warnings
```

测试覆盖公开清单、ZIP下载、条件缓存、磁盘变动后快照一致性、损坏/缺失
文件及非法URL拒绝、资源未启用，另保留原用户接口鉴权和持久化测试。
离线验证不代表真实公网部署、镜像发布或三端集成完成。
