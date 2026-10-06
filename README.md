# rCore-Tutorial-Code

## ch1 实验说明

当前分支 `ch1-api` 用于第 1 章的独立实现。请按 [ch1 API 实验文档](./rcore-ch1-api.md) 实现裸机 Rust 程序的入口 `rust_main()`并在实验报告中分析你的实现与`ch1`参考实现的区别，请在本分支直接提交代码、说明文档及过程日志。

## 实验要求

从 [rCore 实验任务书](https://LearningOS.github.io/rCore-Tutorial-Guide/)的共 8 章中选择 5 章，完成以下任务：

1. **源代码分析与实验报告**：对所选 5 章的源代码进行**静态分析**和**跟踪分析**，撰写实验报告。需要分析的代码位于 `ch{$ID}` 分支。具体分析要求以课上说明为准。
2. **独立实现与对比**：从自己已分析的 5 章中选择 2 章进行独立实现，并在实验报告中比较自己的实现与参考实现的异同。需要独立实现的代码和说明文档位于 `ch{$ID}-api` 分支，完成所选的 2 章即可，可直接在对应分支提交。

（`$ID` 为章节编号，可选1-8）

**代码提交必须包含开发过程的操作日志和 AI 交互日志**。请使用下方介绍的工具记录实验过程，并将日志随实验代码一起提交到实验仓库。

## 实验过程记录工具

课程过程记录：在仓库根目录运行 `python3 course.py`，先自动初始化 AI 会话归档，再打开实验与实时日志。安装要求、日志位置和 Codex 入口见 [实验过程记录说明](https://github.com/leeehh/rCore-Tutorial-Code-api/blob/main/docs/course-recording.md)。

AI过程记录：启动时默认使用 `auto`，可通过 `python3 course.py --agent codex` 指定客户端；也支持 `claude`、`cursor`、`vscode`、`copilot`、`opencode` 和 `all`。会话记录保存到 `.ai/agent-sessions/<agent>/`，文件名包含日期时间，格式为 JSONL。`.ai/agent-sessions/`、`.ai/events/` 和 `.ai/submissions/` 的全部内容须随实验代码一起提交。请同学们不要改动或删除这些记录，提交时会检查这些记录作为考核参考。详细说明见 [AI 会话归档说明](https://github.com/leeehh/rCore-Tutorial-Code-api/blob/main/docs/agent-session-archive.md)。

**Codex 首次使用需要信任 hooks**：安装完成后，在实验仓库根目录的终端运行 `codex`，进入后输入 `/hooks`，找到 `rcore-session-archive` 的 `Stop` 和 `SessionEnd`，分别审阅并选择 **Trust（信任）**。未信任时不会自动保存会话。使用 VS Code Codex 的同学完成后还需重载窗口并新建会话；更新插件后，如提示 hooks 发生变化，请重新审阅并信任。

记录功能与验证：[实验过程记录工具功能说明](https://github.com/leeehh/rCore-Tutorial-Code-api/blob/main/docs/course-monitor-report.md)。main 和各实验分支均提供 `course.py` 及完整运行依赖；安装一次后，切换到 `ch1`–`ch8` 仍会记录。实验分支可使用 `git course logs` 查看日志，使用 `git agent-plugins auto` 再次配置 AI 归档。

## 历史文档参考

### Code

- [Soure Code of labs](https://github.com/LearningOS/rCore-Tutorial-Code)

### Documents

- Concise Manual: [rCore-Tutorial-Guide](https://LearningOS.github.io/rCore-Tutorial-Guide/)

- Detail Book [rCore-Tutorial-Book-v3](https://rcore-os.github.io/rCore-Tutorial-Book-v3/)

### OS API docs of rCore Tutorial Code

- [OS API docs of ch1](https://learningos.github.io/rCore-Tutorial-Code/ch1/os/index.html)
  AND [OS API docs of ch2](https://learningos.github.io/rCore-Tutorial-Code/ch2/os/index.html)
- [OS API docs of ch3](https://learningos.github.io/rCore-Tutorial-Code/ch3/os/index.html)
  AND [OS API docs of ch4](https://learningos.github.io/rCore-Tutorial-Code/ch4/os/index.html)
- [OS API docs of ch5](https://learningos.github.io/rCore-Tutorial-Code/ch5/os/index.html)
  AND [OS API docs of ch6](https://learningos.github.io/rCore-Tutorial-Code/ch6/os/index.html)
- [OS API docs of ch7](https://learningos.github.io/rCore-Tutorial-Code/ch7/os/index.html)
  AND [OS API docs of ch8](https://learningos.github.io/rCore-Tutorial-Code/ch8/os/index.html)
- [OS API docs of ch9](https://learningos.github.io/rCore-Tutorial-Code/ch9/os/index.html)

### Related Resources

- [Learning Resource](https://github.com/LearningOS/rust-based-os-comp2025/blob/main/relatedinfo.md)

### Build & Run

```bash
# setup build&run environment first
$ git clone https://github.com/LearningOS/rCore-Tutorial-Code.git
$ cd rCore-Tutorial-Code
$ git clone https://github.com/LearningOS/rCore-Tutorial-Test.git user
$ git checkout ch$ID
$ cd os
# run OS in ch$ID
$ make run
```

If you want to use docker to build and run, you can use the following command:
```bash
# After clone the `rCore-Tutorial-Test` repository to your local machine, you can use the following command to build and run:
$ make build_docker
$ make docker
```

If you experience network issues when accessing foreign resources such as GitHub in Docker, you can follow the following suggestions according to your stage:

- Docker pull:
  1. use proxy: https://docs.docker.com/reference/cli/docker/image/pull/#proxy-configuration

  2. use available domestic source (self-search)

- Docker build: use proxy https://docs.docker.com/engine/cli/proxy/#build-with-a-proxy-configuration

- Docker run: use proxy option, related operations are similar to `Docker build`, can refer to the relevant materials by yourself


Notice: $ID is from [1-9]

### Grading

```bash
# setup build&run environment first
$ git clone https://github.com/LearningOS/rCore-Tutorial-Code.git
$ cd rCore-Tutorial-Code
$ rm -rf ci-user
$ git clone https://github.com/LearningOS/rCore-Tutorial-Checker.git ci-user
$ git clone https://github.com/LearningOS/rCore-Tutorial-Test.git ci-user/user
$ git checkout ch$ID
# check&grade OS in ch$ID with more tests
$ cd ci-user && make test CHAPTER=$ID
```

Notice: $ID is from [3,4,5,6,8]

## 记录脚本更新

已同步 `course-tool` 的 `6d68289` 版本，支持 `--project` 和 OpenCode。
已有安装需在当前分支拉取更新后重新运行 `python3 course.py`；OpenCode 用户运行 `python3 course.py --agent opencode`。
仅更新运行文件时使用 `python3 course.py install --skip-extension`，再用 `git agent-plugins <客户端>` 刷新客户端 hooks。
运行副本跨章节使用；三类过程记录随代码提交，本地运行文件与客户端配置继续忽略。详见 [记录说明](docs/course-recording.md)。
