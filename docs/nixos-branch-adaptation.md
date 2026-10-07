# 实验分支的 NixOS 开发环境适配

本仓库的 `main` 分支只保存课程过程记录工具和本机（NixOS）的开发环境；各实验
分支 `ch1`–`ch8`、`ch1-api`–`ch8-api` 都是**独立历史**（orphan branch），不包含
这些文件。每开始一章前，需要把 `main` 上的环境提交移植到该章的实验分支。

本文档只是适配说明，保存在 `main` 上，**后续 cherry-pick 时不要带入实验分支**。

## 1. 需要从 `main` 移植的提交

| `main` 提交 | 说明 | 主要文件 |
| --- | --- | --- |
| `f22f335` `build: add NixOS dev shell for the lab environment` | Nix dev shell | `.envrc`、`flake.nix`、`flake.lock`、`rust-toolchain.toml` |
| `d1c3a79` `feat: add pi course-recording extension` | pi 会话归档扩展 | `.pi/extensions/rcore-session-archive/*`、`.pi/session-archive.example.json`、`docs/pi-session-archive.md`、`tests/test_pi_extension.mjs`、`.gitignore` |

`rust-toolchain.toml` 在实验分支上已经存在且与 `main` 内容一致，cherry-pick 时
通常可自动合并；`.gitignore` 也一般能自动合并。

## 2. 每个分支需要额外完成的适配

以下改动**不在** `main` 上，需要在每个实验分支单独完成。

### 2.1 `os/Makefile`

- 增加 `GDB ?= riscv64-unknown-elf-gdb`，并在 `debug`、`gdbclient` 中使用
  `$(GDB)`。NixOS 提供的是多架构 `gdb`，没有 `riscv64-unknown-elf-gdb`，因此
  本地调试用 `make ... GDB=gdb`。
- 把 `env` 目标改成「存在 rustup 才安装」，让 Nix dev shell 下成为 no-op。

### 2.2 `user/` 测试程序

第 2 章及以后需要用户程序（`ch1` 等不加载用户程序的分支可跳过）：

```sh
git clone https://github.com/LearningOS/rCore-Tutorial-Test.git user
printf '\n/user/\n' >> .git/info/exclude   # 本地忽略，避免误提交
```

`rCore-Tutorial-Test` 的 `Makefile` 会用 `TEST`/`CHAPTER`/`BASE` 选择当前章节的
应用；构建产物位于 `user/build/bin/`，由 `os/build.rs` 内联进内核。

## 3. 新开一章的完整步骤

以第 `N` 章为例：

1. `git switch -c chN origin/chN`（独立实现用 `chN-api`）。
2. `git cherry-pick f22f335 d1c3a79`。
3. 按第 2.1 节修改 `os/Makefile`：
   - `ch2` 及以后保留原有的 `@make -C ../user build ...`；
   - `ch1` 等分支该行本就不存在，无需处理。
4. 完成第 2.2 节的 `user` 克隆与排除（如需）。
5. 把上述环境改动 **squash 为一个提交**，例如：
   `chore(chN): set up NixOS development environment`。
6. 验证：`cd os && make build BASE=2 && make run BASE=2`（`BASE` 视章节而定）。

## 4. 已完成记录

| 分支 | 环境提交 | 说明 |
| --- | --- | --- |
| `ch2` | `chore(ch2): set up NixOS development environment` | dev shell + pi 扩展 + Makefile 适配，已 squash 为一个提交 |
| `ch2-api` | `chore(ch2-api): set up NixOS development environment` | 同上 |

`squash` 的提交哈希在分支重写后会变化，这里只记录提交信息。
