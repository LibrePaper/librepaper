# Simplified Chinese glossary (zh)

Locale code: zh. Target: Simplified Chinese, mainland China conventions and software terminology (Microsoft, Apple and Google zh-CN style).

## 1. Register and conventions

- Address: use 你 and 你的 (neutral, friendly, as in most modern web apps). Do not use 您. Avoid addressing the user at all when a sentence reads naturally without a pronoun (for example "无法保存" rather than "你无法保存").
- Buttons and menu items: bare verb or verb-object phrase, no 请, no 吧, no 了 ("保存", "新建项目", "重命名"). Toggle and state labels are nouns or short phrases ("离线", "已解决").
- Status and error messages: short declarative sentences. Success may use 已 ("已保存"). Failure uses 无法 or 失败 ("无法保存", "上传失败"). Do not use exclamation marks.
- Punctuation: always full-width in Chinese sentences: ，。：；？！（）、. Do not use a half-width comma, period, colon or parenthesis next to Chinese text. Inside a fully Latin or code span (a file name, a command, a URL) keep half-width characters.
- Quotation marks: curly full-width style “ ” for double and ‘ ’ for single. Do not use 「」 (that is Traditional/Japanese practice).
- Ellipsis: use "……" (two U+2026 characters) in prose. In button labels that open a dialog, use "…" (one U+2026), matching the English "..." convention in the source. Follow what the English string uses: a trailing "…" in English becomes a trailing "…".
- Enumeration comma: use 、 between short list items inside a sentence.
- No space before or after full-width punctuation. Do not put a space between a Chinese character and a full-width mark.
- Spacing between Chinese and Latin text or numbers: put one half-width space between Chinese characters and Latin words, and between Chinese characters and digits ("共 3 个文件", "导入 BibTeX 文件", "使用 Typst 编译"). Do not add a space between a number and its unit symbol or a percent sign ("5 MB" keeps its space as in Latin; "50%" has none). Do not add a space between a Chinese character and an adjacent full-width punctuation mark. Do not add a space inside a placeholder boundary beyond what this rule asks, and keep placeholders like {count} intact.
- Placeholders and plurals: Chinese has no plural. Write one form ("{count} 个文件"). Always include a measure word: 个 (general), 份 (documents, copies), 条 (comments, items in a list), 次 (times), 位 (people, polite), 项 (settings, tasks).
- Menu paths in sentences: "视图 > PDF", with a space on each side of ">". Menu names are never wrapped in extra quotes.
- Date and time: follow the existing format helpers; if written out, use 年月日 and 24-hour time.
- Do not use traditional characters or Taiwan/Hong Kong terms (for example 軟體, 檔案, 資料夾, 專案). Use mainland terms (软件, 文件, 文件夹, 项目).
- Do not use en dashes or em dashes in the translation. Use commas, colons or parentheses. For ranges use "至" or a plain hyphen between digits only if the English string has a hyphen.

## 2. Menu titles

| English | Chinese | Note |
|---|---|---|
| File | 文件 | |
| Edit | 编辑 | |
| Insert | 插入 | |
| View | 视图 | Microsoft term. Do not use 查看 or 显示 for the menu. The verb "view" in a sentence may be 查看. |
| Settings | 设置 | Not 设定 |
| Help | 帮助 | |
| Tools | 工具 | |
| Format | 格式 | |

Menu path examples: "文件 > 另存为模板", "视图 > HTML", "视图 > PDF". Always use these exact names.

## 3. Term table

| English | Chinese | Note |
|---|---|---|
| project | 项目 | Not 专案 |
| document | 文档 | The thing being written. 文件 is for files on disk. |
| file | 文件 | |
| folder | 文件夹 | |
| main file | 主文件 | |
| new | 新建 | verb form on buttons |
| open (verb) | 打开 | |
| close | 关闭 | |
| save | 保存 | Not 储存 |
| save as | 另存为 | |
| rename | 重命名 | |
| delete | 删除 | |
| duplicate | 创建副本 | For a project or file. Short form 复制 only for clipboard. |
| import | 导入 | |
| export | 导出 | |
| compile | 编译 | LaTeX, Typst and Quarto community word. |
| compiler | 编译器 | |
| build | 构建 | Generic software build. For documents use 编译. |
| render (verb, Markdown/Quarto/HTML) | 渲染 | Use 编译 for LaTeX and Typst. |
| rendering, render (noun) | 渲染结果 | Or 渲染 alone when short. |
| engine (TeX engine) | 引擎 | "XeLaTeX 引擎" |
| preamble | 导言区 | LaTeX community term. |
| package (LaTeX) | 宏包 | Typst packages: 包. In a mixed string use 宏包 for LaTeX and 包 for Typst. |
| document class | 文档类 | |
| macro | 宏 | |
| command (LaTeX) | 命令 | |
| environment (LaTeX) | 环境 | |
| theorem | 定理 | |
| proof | 证明 | |
| lemma | 引理 | |
| definition | 定义 | |
| equation | 公式 | In math context. "方程" only if the string means an equation to solve. |
| formula | 公式 | |
| math | 数学 | |
| symbol | 符号 | |
| figure | 图 | Short label ("图 1"). Longer: 图片 or 图表 depending on meaning. Default: 图. |
| image | 图片 | |
| table | 表格 | Caption number: 表 1. |
| caption | 图注 | Table caption: 表注. Generic: 标题 is not used (clashes with title). |
| label (LaTeX \label) | 标签 | |
| cross-reference | 交叉引用 | |
| reference (bibliography entry) | 参考文献 | |
| citation | 引用 | Insert > Citation: 插入 > 引用. |
| cite key | 引用键 | |
| bibliography | 参考文献 | Biber/BibTeX list. |
| footnote | 脚注 | |
| heading | 标题 | |
| title | 标题 | |
| author | 作者 | |
| abstract | 摘要 | |
| section | 章节 | Or 节 in LaTeX-specific text. |
| paragraph | 段落 | |
| list | 列表 | |
| outline | 大纲 | Microsoft term. |
| table of contents | 目录 | |
| word count | 字数统计 | Chinese counts characters. Use 字数 for the short form. |
| page | 页 | |
| font | 字体 | |
| font size | 字号 | |
| line height | 行高 | |
| line number | 行号 | |
| spelling | 拼写 | |
| grammar | 语法 | |
| spell check | 拼写检查 | |
| grammar check | 语法检查 | |
| track changes | 修订 | Word term. |
| change (an edit) | 修改 | |
| suggestion | 建议 | |
| accept | 接受 | |
| reject | 拒绝 | |
| accept all | 全部接受 | |
| reject all | 全部拒绝 | |
| comment (noun) | 批注 | Word term for review comments. Use 评论 only for chat or social-style comment streams. In this app use 批注. |
| comment (verb) | 批注 | |
| commenter | 批注者 | The role. |
| resolve | 解决 | |
| resolved | 已解决 | |
| reopen | 重新打开 | |
| reply | 回复 | |
| mention | 提及 | |
| share | 分享 | Mainland software favors 分享. |
| share link | 分享链接 | |
| copy link | 复制链接 | |
| permission | 权限 | |
| access | 访问权限 | |
| owner | 所有者 | |
| editor (role) | 编辑者 | |
| reader (role) | 阅读者 | |
| editor (the text editor) | 编辑器 | |
| collaborator | 协作者 | |
| anonymous | 匿名 | |
| history | 历史记录 | |
| version | 版本 | |
| bookmark | 书签 | Named version marker. |
| restore | 恢复 | Also for trash items. |
| compare | 比较 | |
| diff | 差异 | |
| branch | 分支 | |
| commit | commit | Kept in English, noun and verb. |
| push | push | Kept in English (verb and noun). Chinese Git users say "push". |
| pull | pull | Kept in English. |
| merge | 合并 | |
| conflict | 冲突 | |
| repository | 仓库 | Git. |
| remote | 远程 | |
| clone | 克隆 | |
| sync | 同步 | |
| synced | 已同步 | |
| upload | 上传 | |
| download | 下载 | |
| trash | 回收站 | |
| move to trash | 移到回收站 | |
| empty trash | 清空回收站 | |
| template | 模板 | |
| tag | 标签 | Same word as LaTeX label. Context separates them. In the tag UI of the project list use 标签. For LaTeX \label use 标签 too, but add 引用标签 only if both appear in one string. |
| favorites | 收藏夹 | The built-in tag/list. The action "favorite" is 收藏. |
| search | 搜索 | |
| find | 查找 | |
| replace | 替换 | |
| undo | 撤销 | |
| redo | 重做 | |
| copy | 复制 | |
| cut | 剪切 | |
| paste | 粘贴 | |
| select all | 全选 | |
| sign in | 登录 | |
| sign out | 退出登录 | |
| account | 账户 | Mainland: 账户 or 账号. Use 账户. Username/handle: 用户名. |
| profile | 个人资料 | |
| password | 密码 | |
| email | 电子邮件 | |
| token | 令牌 | Access/agent tokens. |
| API key | API 密钥 | |
| quota | 配额 | |
| storage | 存储空间 | |
| usage | 用量 | |
| limit | 上限 | |
| backup | 备份 | |
| agent | 智能体 | AI agent. Mainland community uses 智能体. |
| AI | AI | Latin letters kept. |
| chat | 聊天 | |
| message | 消息 | |
| assistant | 助手 | |
| permission request | 权限请求 | |
| Run code | 运行代码 | |
| run | 运行 | |
| output | 输出 | |
| error | 错误 | |
| warning | 警告 | |
| log | 日志 | |
| diagnostics | 诊断 | |
| preview | 预览 | |
| source | 源码 | Editor source view. "Source" as origin: 来源. |
| source mode | 源码模式 | |
| split view | 分栏视图 | |
| fullscreen | 全屏 | |
| zoom | 缩放 | |
| fit width | 适应宽度 | |
| page view | 页面视图 | |
| desktop app | 桌面应用 | Not 桌面端 in UI. |
| this computer | 这台电脑 | |
| this browser | 此浏览器 | |
| browser | 浏览器 | |
| offline | 离线 | |
| online | 在线 | |
| connected | 已连接 | |
| disconnected | 已断开连接 | |
| reconnect | 重新连接 | |
| keyboard shortcut | 快捷键 | |
| key bindings | 键位绑定 | Vim/Emacs keymaps. |
| theme | 主题 | |
| light | 浅色 | |
| dark | 深色 | |
| language | 语言 | |
| appearance | 外观 | |
| notification | 通知 | |
| privacy | 隐私 | |
| terms | 条款 | |
| admin | 管理员 | |
| moderation | 审核 | |
| report (abuse) | 举报 | |
| block | 封禁 | |
| hidden | 已隐藏 | |
| public | 公开 | |
| private | 私有 | |
| loading | 正在加载 | |
| retry | 重试 | |
| cancel | 取消 | |
| confirm | 确认 | |
| OK | 确定 | |
| yes / no | 是 / 否 | |
| learn more | 了解更多 | |

## 4. Ambiguous short labels

| English | Chinese | Sense in this app |
|---|---|---|
| Open | 打开 | Open a project or file. As an adjective (include is open): 已打开. |
| Run | 运行 | Execute code. Do not use 执行 on buttons. |
| Fork | 创建副本并编辑 | Landing page action on a shared document. Git-sense fork: 派生. |
| Split | 分栏 | Splitting the view into panes. Not 拆分 (which is for splitting a file). |
| Draft | 草稿 | |
| Label | 标签 | LaTeX/Typst label. |
| Caption | 图注 | Table caption 表注. |
| Source | 源码 | The editable text. |
| Preview | 预览 | |
| Render | 渲染 | Markdown/HTML/Quarto. 编译 for LaTeX and Typst. |
| Find | 查找 | |
| Replace | 替换 | |
| Format | 格式 | File/output format. The command "format code": 格式化. |
| Insert | 插入 | |
| Close | 关闭 | |
| Clear | 清除 | Clear a field or list. Not 清空 unless emptying a container (清空回收站). |
| Reset | 重置 | Settings and passwords. |
| Revoke | 撤销 | Tokens and links. Same word as undo; context is clear. |
| Star | 星标 | |
| Pin | 固定 | Unpin: 取消固定. |
| Lock | 锁定 | |
| Unlock | 解锁 | |
| Back | 返回 | |
| Done | 完成 | |
| Apply | 应用 | |
| Keep | 保留 | |
| Discard | 放弃 | Discard changes: 放弃更改. |
| Move | 移动 | |
| Copy | 复制 | |
| Link | 链接 | Also the verb: 链接到. |
| Note | 注意 | Callout label. A note attached to something: 备注. |
| Status | 状态 | |
| Default | 默认 | |
| Custom | 自定义 | |
| System | 系统 | Theme follow-system option: 跟随系统. |
| Automatic | 自动 | |

## 5. Kept in English

- Product and format names: LibrePaper, LaTeX, Typst, Quarto, Markdown, HTML, PDF, DOCX, Word, Zotero, GitHub, Git, Harper, Pandoc, BibTeX, Biber, TeX Live, XeLaTeX, pdfLaTeX, LuaLaTeX, Vim, Emacs, Claude Code, ORCID, OIDC, MCP, ACP, API, URL, ID.
- Git verbs commit, push, pull stay in English. Branch, merge, clone, repository are translated.
- Keyboard key names stay Latin: Ctrl, Alt, Shift, Enter, Esc, Tab, Cmd, Option. Do not translate them. Arrow keys may stay as symbols.
- File extensions, commands, code, paths, URLs and placeholders in braces stay unchanged.
- Chinese LaTeX community terms listed above (编译, 宏包, 导言区, 文档类, 交叉引用, 参考文献) are the preferred forms.

## 6. Units and numbers

- Byte units stay Latin: B, KB, MB, GB, with a half-width space before the unit ("50 MB").
- Use Western (Arabic) digits and a period decimal separator. Thousands separator is a comma as in the English source.
- Percent has no space: "50%".
- Counts: number + space + measure word + noun: "3 个文件", "12 条批注", "2 位协作者", "5 个版本". Zero is written the same way ("0 个文件").
- Word count: Chinese text is counted in characters. Label the number as 字数, and use 词 only if the string clearly counts Latin words.
