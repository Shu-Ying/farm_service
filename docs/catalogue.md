# 作物目录独立维护与发布

`catalogue/source/` 是日常维护的正式来源，不再读取 Farm 仓库或旧作物库。
首次迁出保留 202 条作物的全部业务字段、768 张图片的原始字节及现有布局。
原 `assets.zip` 和小型 `land-soils.zip` 字节不变，旧迁移基线保持原样。

## 源目录

- `plants.json`：根对象仅含 `schema: 1` 和 `plants` 数组。
- `metadata.json`：维护画布、布局、定位、panel 等绘图配置；发布工具重建
  `plants` 和 `images`，不要手工修改这两个生成字段。
- `images/<原 ZIP 路径>`：PNG/JPEG 等原始图片，每个文件对应同名 ZIP 成员。
- `land-soils.zip`：精确包含 `soil/红土地.png`、`soil/黑土地.png`、
  `soil/金土地.png` 三张可解码图片。

作物 `id` 为永久编号，不能重排、复用、删除或更名。名称唯一且不超过
128 字符，不能为 `.`、`..` 或包含 `/`、`\`。数值仅允许 64 位有符号
范围内非负整数，`id`、`time`、`crop`、
`harvest` 必须为正；`time` 为小时，转换为秒仍须在范围内。
`level/buy/vipBuy/experience/price` 分别控制等级、种子普通币价格、点券价格、
经验和收获物售价。`isVip`、`isBuy` 和可选 `enabled` 仅允许 0 或 1；
可选 `crop_type` 默认为 1，只有类型 1 可新种植。

`phase` 为逗号分隔的正整数秒值，按首次出现去重后须严格递增且至少两项；
它与 `general/offsetH/offsetW/offsetX/offsetY` 一并生成展示元数据。
其他已有字段原样保留。图片原有缺图占位行为保留，但不能删除已经发布的
图片路径；新增作物应同时补充所需阶段图片。

发布工具独立校验完整展示契约：30 个地块位置、schema 4 作物放置、
面板字段及贴图、虫动画、六主题装饰、画布与图片尺寸，以及所有必需
背景和通用图片引用。schema 不接受布尔值，坐标不接受布尔值或非有限数。

## 编辑、检查、生成

需要 Python 3.10+ 和 Pillow；发布脚本不安装依赖、不访问网络或玩家数据库。
从本仓库根目录运行：

```text
python scripts/prepare_catalogue.py --source catalogue/source --output catalogue --version catalogue-20261026-1 --check
python -m unittest discover -s tests -p test_prepare_catalogue.py
```

`--check` 只读核对源与当前最新发布版本，源已修改时会拒绝，且不会生成
目录或替换清单。变更后先运行测试，再显式生成：

```text
python scripts/prepare_catalogue.py --source catalogue/source --output catalogue --version catalogue-20261027-1
python scripts/prepare_catalogue.py --source catalogue/source --output catalogue --version catalogue-20261027-1 --check
```

例如把已有条目的 `price` 改为新售价，或新增从未使用过的 ID；不要把
旧 ID 改成另一个作物。`isBuy=0` 表示禁售，`enabled=0` 禁止新购买和新播种。
生成时对照当前已校验版本，拒绝删除/更名旧 ID 或丢失已有图片路径。

产物位于 `catalogue/releases/<release>/`，包含清单和精确四个文件：
`plants.json`、`metadata.json`、`assets.zip`、`land-soils.zip`。
生成完整目录后，最后原子替换 `catalogue/manifest.json` 发现入口；失败
不会切换最新版本。已存在版本必须逐字节相同，不能原地覆盖。相同内容
重复生成须沿用相同版本标签；仅更换标签不能覆盖相同内容的历史清单。
已经发布的标签也不能用于内容不同的新版本，修改内容时必须使用新标签。

发布操作按输出目录同时使用进程内线程锁和系统文件锁，锁覆盖读取最新
版本、兼容性检查与最终替换，防止并发旧发布删除刚新增的 ID。系统锁在
进程退出时自动释放；`.publish.lock` 是应保留的空状态锁文件，不应删除，
仓库已忽略它。`--check` 保持只读，不创建或修改锁文件。

ZIP 使用固定成员顺序、2020-01-01 时间、Unix 0644 文件权限和 STORED 格式，
保留图片原始字节。输出不依赖文件修改时间或 Farm 相邻工作区。
临时 `.prepare-*` 目录不是有效发布版本，可在确认没有发布进程后清理。

## 清单协议与 HTTP

清单严格包含 `schema/version/release/files`：schema 固定 1；version 为
1～64 个字母数字或 `._-`；files 精确对应四文件，每项仅含正整数
`size_bytes` 和 64 位小写 `sha256`。release 为 files 对象经键排序、紧凑
UTF-8 JSON 序列化后的 SHA256，不接受 URL 或自定义路径。

清单最大 16 KiB；plants 2 MiB；metadata 4 MiB；各 ZIP 64 MiB；两个 ZIP
解压后图片总量不超过 256 MiB。源发布工具校验业务数据与图片，Rust 服务
启动校验各历史清单关系、文件大小和摘要；客户端也校验完整版本。
这些摘要检测不一致和损坏，不构成发布账号被接管时的真实性保证。

设置 `FARM_CATALOGUE_DIR=./catalogue` 后，服务启动加载最新清单及所有
历史版本为不可变内存快照：

| 接口 | 行为 |
|---|---|
| `GET /v1/catalogue/manifest.json` | 最新清单，`no-cache`，支持 ETag |
| `GET /v1/catalogue/releases/{release}/{file}` | 精确白名单四文件和该版本 `manifest.json`；一年 immutable 缓存，支持 ETag |

接口公开只读，不需要用户资料接口令牌。未配置目录时返回 503；未知版本
和文件返回 404；无效目录或摘要导致启动失败，并写入既有日志。
运行中不重读磁盘，发布后须重启 Service。旧素材/字体接口及用户资料鉴权
保持独立。所有历史版本都会驻留内存，部署前须按版本数量规划内存。

GitHub/Gitea 同样从本仓库 raw 根路径的 `catalogue/manifest.json` 和
`catalogue/releases/<release>/<file>` 获取相同字节。应先发布完整版本目录，
最后发布发现入口，保留历史版本；本工具仅准备本地文件，不执行提交、
推送或部署。真实远端下载与生产重启需在发布后另行验证。

## 重启生效与兼容回滚

机器人仅在启动时检查一次更新，同一次运行使用一个完整版本，失败优先
回退已校验缓存，再回退插件初始版本。若选定版本无法覆盖玩家已持有或
已种植的 ID，应拒绝业务初始化，恢复兼容版本后再启动，不能清空玩家数据。

新播种使用本次启动目录；已经播种的地块继续使用原结算快照。出售使用
当前目录售价，调价也会影响旧库存。回滚应从当前源发布一个新兼容版本：
恢复旧数值，但保留期间新增的全部 ID、名称及图片，需要下架时使用禁售/
禁播字段。不能把最新清单直接退回缺少新 ID 的老版本。

Farm 随包的轻量 bootstrap 由显式同步工具从本地完整 release 刷新，日常
Service 发布不要求同时改 Farm；三个机器人分发包继续不捆绑大型素材 ZIP。
字体保持独立发布，不纳入四文件目录版本。
