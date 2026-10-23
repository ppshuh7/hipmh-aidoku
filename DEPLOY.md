# 让 Aidoku 自己"点一下更新" — 部署手册

> 原理：Aidoku 的「浏览」页有个 **更新** 区（`BrowseViewModel.loadUpdates()`），它会读你添加的
> **图源列表 URL（list.json）**，把列表里的 `version` 和你手机上已装的版本比：
> `info.version > installedSource.version` 就出现在"更新"里，点一下它从 `downloadURL` 下载 .aix
> 并覆盖安装。所以只要 list.json 有个**固定网址**，以后发布新版 = 改 version + push，
> 手机端点更新即可，不用 AirDrop。

---

## 仓库里已经准备好的东西

`source-list/repo/`（推上去就是这个仓库的全部内容）：

```
.github/workflows/build.yml    # push 即构建 wasm → 打包 .aix → 生成 list.json → 部署 gh-pages
src/zh.hipmh7/                 # 图源源码（Cargo.lock 一起带上，锁死依赖版本）
aidoku-rs-main/                # aidoku-rs 0.3 @ b081870，Cargo.toml 里的本地路径依赖指向它
dist/                          # 本地已按 CI 的规则预生成的一份产物（可直接用 gh-pages 手动上传）
```

CI 的产物（gh-pages 根目录）：

| 文件 | 作用 |
|---|---|
| `list.json` | 你添加到 Aidoku 的图源列表 |
| `zh.hipmh-v11.aix` | 带版本号的实际源包（**每次发版文件名都变**，避免 CDN 缓存旧包） |
| `zh.hipmh.aix` | 固定名副本（备用） |
| `icon.png` | 列表图标 |
| `index.min.json` | 参考仓库用的扁平索引（非必需，留着兼容） |

关键细节：`list.json` 里的 `downloadURL` / `iconURL` 写的是**相对文件名**。Aidoku 的
`ExternalSourceInfo.fileURL` 用 `URL(string: relativeTo: listUrl)` 解析，所以列表放哪都能用；
文件名带版本号还能绕开 jsdelivr/GitHub 的缓存（缓存旧 list.json 只影响"发现更新"，
缓存旧 .aix 才会装错版本）。

✅ 本地已验证：用 `python -m http.server` 起一个静态服务，按上面的解析规则取 `list.json`
→ 解析出 `zh.hipmh-v11.aix` → 下载 79,708B → 解包 `Payload/{main.wasm,source.json,icon.png}`
→ `source.json` 里 `version=11`。整条链路通。

---

## 你需要做的（只有这几步）

### 1. 建一个公开仓库

GitHub → New repository：

- 名称随意，例如 `hipmh-aidoku`
- **Public**（Pages 要求公开；私有仓库要 Pages 就得 Pro）
- **不要**勾选 "Add a README"（要空仓库，否则 push 会冲突）

### 2. 把 `source-list/repo/` 的内容推上去

在 `D:\Project Field\AIDOKU` 下执行（把 `<你的用户名>`、`<仓库名>` 换掉）：

```bash
cd "source-list/repo"
git init -b main
git add .
git commit -m "zh.hipmh v11: incremental chapter refresh"
git remote add origin https://github.com/<你的用户名>/<仓库名>.git
git push -u origin main
```

> `aidoku-rs-main/` 有几十 MB，首次 push 会慢一点，正常。

### 3. 打开 Pages（用 Actions 部署）

仓库 → **Settings → Pages** → Source 选 **Deploy from a branch** → Branch 选 **gh-pages** /
**/(root)** → Save。

（`gh-pages` 分支由 CI 第一次跑完自动创建，所以先做第 4 步再回来选也行。）

### 4. 让它跑一次

仓库 → **Actions** → 左侧 "Build and deploy Aidoku source" → **Run workflow**（或随便 push 一次）。
跑完看 `gh-pages` 分支上是否有 `list.json`。

### 5. 在 Aidoku 里添加列表（只需一次）

Aidoku「浏览」→ 右上角 **+** → **图源列表**（或设置里的源列表），粘贴：

```
https://<你的用户名>.github.io/<仓库名>/list.json
```

添加后，「浏览」页就会出现 **嬉皮漫画**；当前手机装的是 v11，所以此时"更新"区是空的（已是最新）。

> 国内直连 `github.io` 可能不稳。如果 App 里加载失败，换成镜像前缀（实测都能取到 GitHub 文件）：
> - `https://gh-proxy.com/https://<你的用户名>.github.io/<仓库名>/list.json`
> - `https://ghproxy.net/https://<你的用户名>.github.io/<仓库名>/list.json`
>
> 镜像只加在 list.json 的 URL 上即可，里面的 .aix 是相对路径，会跟着同一个镜像走。

---

## 以后怎么发版（三步走，不再 AirDrop）

1. 改源码，`src/zh.hipmh7/res/source.json` 的 `info.version` +1（12、13…）
2. 本地验证：`decode_test` 三套测试 → `cmd /c src\zh.hipmh7\build.cmd`
3. 把改动同步到仓库并 push：

```bash
cd "source-list/repo"
git add -A
git commit -m "v12"
git push
```

CI 自动构建、打新文件名、更新 `list.json` 的 version → 手机 Aidoku「浏览 → 更新」
里出现「嬉皮漫画」，点 **获取** 即完成升级（App 是显式点击更新，不会偷偷装）。

---

## 排障

| 现象 | 检查 |
|---|---|
| 更新区一直空 | list.json 的 `version` 是否 > 已装版本；仓库 Actions 是否跑成功；手机上 list URL 能否打开 |
| 图源卡片显示"图源列表缺失"警告 | list.json 里 `id` 必须与包内 `source.json` 的 `id`（`zh.hipmh`）**完全一致** |
| 点了获取但版本没变 | 检查 gh-pages 上 `zh.hipmh-v<N>.aix` 的 `Payload/source.json` 里 version 是否真的 +1 |
| 加载列表报 JSON 错 | `list.json` 必须是 `{"name":..., "sources":[...]}` 结构（不是数组）；旧的扁平数组格式是 legacy 格式，也能读但会走兼容分支 |
| 国内打不开 github.io | 用上面两个镜像前缀之一 |
