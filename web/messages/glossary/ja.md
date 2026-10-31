# Japanese glossary (ja)

## 1. Register and conventions

- Sentences use the polite です/ます style (丁寧語). Do not use the plain だ/である style in the interface. Do not use honorific or humble keigo beyond standard polite forms (no お客様, no ご〜くださいませ). Address the user without a pronoun (never あなた unless unavoidable).
- Buttons, menu items, tab labels and column headings use short noun forms or dictionary-stem verb phrases, as in mainstream Japanese software: 保存, 削除, 名前を変更, ファイルを開く, コメントを追加. Do not end buttons with ます or ください.
- Confirmation questions use です/ます: 「このプロジェクトを削除しますか?」.
- Status messages and errors use です/ます or a noun phrase: 「保存しました」, 「接続できません」, 「読み込めませんでした」. Prefer 「〜できませんでした」 over 「〜に失敗しました」 when a natural option exists. Avoid 「〜してください」 chains; one 「してください」 per message at most.
- Progress and loading text: 「読み込み中…」, 「保存中…」.
- Placeholders such as {count}, {name} stay unchanged. Counters follow the number: {count} 件, {count} 個, {count} 人. Japanese has no plural forms, so plural and singular variants use the same wording.

### Punctuation and typography

- Use full-width punctuation in Japanese sentences: 。 、 ！ ？ （ ） 「 」 『 』 ： ・ 〜.
- Sentence ends take 。 in full sentences. Short labels, buttons, menu items, titles and tooltips that are not full sentences take no final 。.
- Use the comma 、 (not ，) and period 。 (not ．).
- Quotation marks are 「 」. Nested quotes use 『 』. Do not use curly or straight English quotes around Japanese text.
- Parentheses are full-width （ ） when the content is Japanese. When the parenthesised content is entirely Latin text or a number, half-width ( ) is acceptable, but be consistent inside one string.
- Colon: use full-width ： after a label in running text. Keep half-width : in times, ratios and code.
- Question and exclamation marks: use full-width ？ and ！ in sentences. Avoid ！ in UI text unless the English has it.
- Ellipsis: use the single character … (U+2026) for "in progress" and for commands that open a dialog (名前を付けて保存…). Do not use 「...」 or 「……」.
- Middle dot ・ separates items in a list (カット・コピー・貼り付け) and katakana compound words with two foreign words that need a break (e.g. ブックマーク・リスト is avoided; prefer a single word).
- Wave dash: use 〜 (U+301C) for ranges in prose, or the word から/まで. Never use an en dash or em dash. For ranges of numbers use 〜 with no spaces (例: 1〜5).
- Do not use half-width katakana. Do not use full-width Latin letters or full-width digits. Digits and Latin letters are always half-width.

### Spacing between Japanese and Latin text or numbers

- Do not insert spaces between Japanese text and half-width Latin letters or numbers in running text: 「PDFを開く」, 「3件の変更」, 「Gitに接続」. This follows Microsoft, Apple and Google Japanese style.
- No space between a placeholder and an adjacent Japanese character: 「{count}件」, 「{name}を開く」.
- Between two Latin tokens the normal space stays (write 「Claude Codeを使う」, not 「Claude Code を使う」).
- Never add a space after full-width punctuation.

### Katakana conventions

- Follow current Microsoft Japanese style: keep the trailing long-vowel mark ー on words of three or more morae that end in -er, -or, -ar: フォルダー, ユーザー, ブラウザー, エディター, サーバー, コンピューター, プレビュー, ビューアー, プロバイダー, クリップボード (no ー needed), パラメーター, レンダラー. Apply this to the whole catalogue without exception so that batches agree.
- Exceptions kept short even under this rule: 「メニュー」 (always with ー), 「コマンド」, 「ツール」. Words ending in a long -ー sound that Microsoft always writes with ー keep it.
- Use the common spellings: ファイル, フォルダー, ウィンドウ, ダイアログ, キャンセル, アカウント, パスワード, ログイン (or サインイン, see table), ダウンロード, アップロード, ブックマーク, テンプレート, ショートカット, キーボード, スクリーン. Do not use ディレクトリ; use フォルダー.
- Use ヴ only in established words; prefer バ/ビ/ブ (e.g. ボリューム not ヴォリューム).
- Do not mix hiragana, kanji and katakana spellings of the same term across batches. The term table fixes the choice.

## 2. Menu titles

Menu titles are noun forms. Use these exact words everywhere, including inside sentences.

| English | Japanese |
|---|---|
| File | ファイル |
| Edit | 編集 |
| Insert | 挿入 |
| View | 表示 |
| Format | 書式 |
| Tools | ツール |
| Settings | 設定 |
| Help | ヘルプ |
| Window | ウィンドウ |

Menu paths in sentences use the full-width-friendly separator " > " exactly as in English, with the Japanese menu titles and no extra decoration: 「表示 > PDF」, 「ファイル > 名前を付けて保存」. Keep a single half-width space on each side of ">". Wrap the path in no quotes unless the English does (then 「 」).

## 3. Term table

| English | Japanese | Note |
|---|---|---|
| project | プロジェクト | |
| file | ファイル | |
| folder | フォルダー | Microsoft style with ー |
| main file | メインファイル | |
| document | ドキュメント | Use 文書 only in prose about written text, 論文 for paper/article |
| paper / article | 論文 | |
| preview | プレビュー | Noun and menu label |
| render (verb, general) | レンダリング | Use for HTML/Typst/Quarto output |
| render (noun) | レンダリング結果 | |
| compile | コンパイル | LaTeX community term, also for Typst |
| compile error | コンパイルエラー | |
| source | ソース | |
| source code | ソースコード | |
| preamble | プリアンブル | LaTeX |
| package (LaTeX) | パッケージ | LaTeX community term |
| document class | ドキュメントクラス | |
| bibliography | 参考文献 | Section or list. BibTeX file: 文献データベース is not used; say BibTeX ファイル without space as BibTeXファイル |
| citation | 引用 | |
| citation key | 引用キー | |
| cross-reference | 相互参照 | |
| reference | 参照 | |
| theorem | 定理 | |
| lemma | 補題 | |
| proof | 証明 | |
| definition | 定義 | |
| figure | 図 | Also in captions |
| table | 表 | |
| equation | 数式 | 方程式 only for mathematical equations in prose |
| formula | 数式 | |
| inline math | インライン数式 | |
| display math | ディスプレイ数式 | |
| caption | キャプション | |
| label (LaTeX \label) | ラベル | |
| heading | 見出し | |
| section | セクション | |
| chapter | 章 | |
| footnote | 脚注 | |
| appendix | 付録 | |
| abstract | 概要 | 要旨 is the formal academic form; use 概要 in the UI |
| table of contents | 目次 | |
| outline | アウトライン | |
| word count | 文字数 | Japanese counts characters; use 文字数 as the label and 単語数 only for Latin word counts |
| track changes | 変更履歴 | Microsoft Word term (変更履歴の記録) |
| suggestion | 提案 | |
| suggesting (mode) | 提案モード | |
| change (an edit) | 変更 | |
| accept | 承諾 | Word uses 承諾; button label 承諾 |
| reject | 却下 | Word uses 元に戻す for rejecting; this app uses 却下 |
| accept all | すべて承諾 | |
| reject all | すべて却下 | |
| comment (noun) | コメント | |
| comment (verb) | コメントする | |
| resolve | 解決 | |
| reopen | 再オープン | |
| reply | 返信 | |
| mention | メンション | |
| share | 共有 | |
| share link | 共有リンク | |
| link | リンク | |
| owner | 所有者 | |
| editor (role) | 編集者 | |
| commenter | コメント投稿者 | Short form コメント可 in tight space |
| reader | 閲覧者 | |
| role | ロール | Use 権限 for permission |
| permission | 権限 | |
| access | アクセス | |
| collaboration | 共同編集 | |
| history | 履歴 | |
| version | バージョン | |
| named version | 名前付きバージョン | |
| bookmark (history) | ブックマーク | |
| restore | 復元 | |
| revert | 元に戻す | |
| branch | ブランチ | Git; keep the katakana |
| commit | コミット | Keep in katakana, as the Japanese Git community does |
| push | プッシュ | Katakana; the Git panel may show the English command name in code |
| pull | プル | Katakana |
| repository | リポジトリ | |
| clone | クローン | |
| merge | マージ | |
| conflict | 競合 | |
| sync | 同期 | |
| synchronize (verb) | 同期する | |
| upload | アップロード | |
| download | ダウンロード | |
| export | エクスポート | |
| import | インポート | |
| trash | ごみ箱 | |
| delete | 削除 | |
| delete permanently | 完全に削除 | |
| empty trash | ごみ箱を空にする | |
| rename | 名前の変更 | Menu: 名前を変更 |
| duplicate | 複製 | |
| copy | コピー | |
| cut | 切り取り | |
| paste | 貼り付け | |
| undo | 元に戻す | |
| redo | やり直し | |
| select all | すべて選択 | |
| save | 保存 | |
| save as | 名前を付けて保存 | |
| new | 新規 | Button: 新規作成 |
| new project | 新規プロジェクト | |
| new file | 新規ファイル | |
| new folder | 新しいフォルダー | |
| search | 検索 | |
| find | 検索 | See ambiguous labels |
| replace | 置換 | |
| search settings | 設定を検索 | |
| template | テンプレート | |
| tag | タグ | |
| favorites | お気に入り | |
| star | スター | Use お気に入り for the state |
| token | トークン | |
| agent token | エージェントトークン | |
| quota | 容量 | Use 上限 for a hard limit |
| storage | ストレージ | |
| usage | 使用量 | |
| backup | バックアップ | |
| account | アカウント | |
| sign in | サインイン | Microsoft style; ログイン is also common but keep サインイン throughout |
| sign out | サインアウト | |
| profile | プロフィール | |
| handle | ハンドル | Account handle; keep as ハンドル |
| password | パスワード | |
| settings | 設定 | |
| preferences | 環境設定 | Only when the English says Preferences |
| theme | テーマ | |
| dark / light | ダーク / ライト | |
| appearance | 外観 | |
| font | フォント | |
| font size | フォントサイズ | |
| line height | 行の高さ | |
| key bindings | キーバインド | |
| keyboard shortcut | キーボードショートカット | |
| shortcut | ショートカット | |
| editor (the text editor) | エディター | |
| editor pane | エディターペイン | |
| spelling | スペルチェック | |
| grammar | 文法 | |
| proofreading | 校正 | |
| diagnostics | 診断 | |
| warning | 警告 | |
| error | エラー | |
| problem | 問題 | |
| agent | エージェント | |
| AI agent | AIエージェント | |
| assistant | アシスタント | |
| chat | チャット | |
| message | メッセージ | |
| Run code | コードを実行 | Top-bar switch label; 実行 alone for the button |
| run | 実行 | |
| stop | 停止 | |
| cancel | キャンセル | |
| desktop app | デスクトップアプリ | |
| this computer | このコンピューター | |
| this browser | このブラウザー | |
| browser | ブラウザー | |
| offline | オフライン | |
| online | オンライン | |
| connected | 接続済み | |
| connecting | 接続中 | |
| reconnecting | 再接続中 | |
| companion | コンパニオン | Only if the English string says companion |
| engine | エンジン | |
| format | 形式 | File or output format; menu Format is 書式 |
| output | 出力 | |
| PDF preview | PDFプレビュー | |
| page | ページ | |
| zoom | 拡大縮小 | Label; zoom in 拡大, zoom out 縮小 |
| fullscreen | 全画面表示 | |
| sidebar | サイドバー | |
| panel | パネル | |
| dialog | ダイアログ | |
| tab | タブ | |
| toolbar | ツールバー | |
| menu | メニュー | |
| command palette | コマンドパレット | |
| symbol palette | 記号パレット | |
| math palette | 数式パレット | |
| symbol | 記号 | |
| special character | 特殊文字 | |
| image | 画像 | |
| list | リスト | Bulleted: 箇条書き, numbered: 番号付きリスト |
| bulleted list | 箇条書き | |
| numbered list | 番号付きリスト | |
| bold | 太字 | |
| italic | 斜体 | |
| underline | 下線 | |
| code (inline) | コード | |
| quote | 引用文 | Avoid confusion with 引用 (citation) |
| spell check language | 言語 | |
| size | サイズ | |
| name | 名前 | |
| title | タイトル | |
| description | 説明 | |
| date | 日付 | |
| today / yesterday | 今日 / 昨日 | |
| administrator | 管理者 | |
| demo | デモ | |
| beta | ベータ | |

## 4. Ambiguous short labels

| English | Sense in this app | Japanese |
|---|---|---|
| Open | open a file or project | 開く |
| Open | open a panel or dialog | 開く |
| Open | state "open" (comment thread) | 未解決 |
| Run | execute code | 実行 |
| Fork | copy a document as an independent line | フォーク |
| Split | split the editor and preview panes | 分割 |
| Draft | unsent comment, unsaved version | 下書き |
| Label | LaTeX \label or a named version label | ラベル |
| Label | a tag on a project | タグ |
| Caption | figure or table caption | キャプション |
| Source | source text of a document | ソース |
| Source | origin of a template | 取得元 |
| Preview | the rendered view | プレビュー |
| Render | produce the output | レンダリング |
| Render | compile for LaTeX and Typst | コンパイル |
| Find | search within the document | 検索 |
| Find | the Find and Replace tool | 検索と置換 |
| Replace | substitute text | 置換 |
| Replace | swap a file for another | 置き換え |
| Format | menu title | 書式 |
| Format | file format | 形式 |
| Format | reformat code (verb) | 整形 |
| Insert | menu title | 挿入 |
| Insert | add an item at the cursor (button) | 挿入 |
| Close | close a dialog or panel | 閉じる |
| Close | close brackets (editor setting) | 括弧を閉じる |
| Clear | remove a value or selection | クリア |
| Clear | remove all items | すべて削除 |
| Reset | restore defaults | リセット |
| Revoke | cancel a link or token | 取り消し |
| Star | mark a favorite | お気に入りに追加 |
| Pin | keep an item at the top | ピン留め |
| Lock | lock the preview on a file | ロック |
| Unlock | | ロック解除 |
| Back | go to the previous screen | 戻る |
| Done | finish an operation | 完了 |
| Apply | confirm a setting | 適用 |
| Keep | keep a version or a change | 保持 |
| Discard | throw away changes | 破棄 |
| Move | move a file | 移動 |
| Copy | copy text or a file | コピー |
| Copy | copy of a project (name suffix) | コピー |
| Link | URL link | リンク |
| Link | create a link (verb) | リンクを作成 |
| Note | explanatory text or tip | 注 |
| Note | a Markdown or task note | メモ |
| Status | state of a job | 状態 |
| Status | tool or service status page | ステータス |
| Default | default value | 既定 |
| Custom | user-defined | カスタム |
| System | follows the operating system | システム |
| Automatic | chosen by the app | 自動 |
| Continue | proceed | 続行 |
| Next / Previous | step navigation | 次へ / 戻る |
| Yes / No | | はい / いいえ |
| OK | | OK |
| Edit | change something (verb) | 編集 |
| Edit | role | 編集者 |
| Select | choose | 選択 |
| Show / Hide | | 表示 / 非表示 |
| Add | | 追加 |
| Remove | take out of a list | 削除 |
| Remove | take a tag off a project | 外す |
| Create | | 作成 |
| Rename | | 名前を変更 |
| Details | | 詳細 |
| More | | その他 |

## 5. Kept in English

Keep these exactly as written, in Latin letters, with no katakana equivalent:

- LibrePaper, LaTeX, TeX, Typst, Quarto, Markdown, HTML, CSS, PDF, DOCX, Word, Zotero, GitHub, Git, Harper, Pandoc, BibTeX, Biber, TeX Live, Vim, Emacs, Claude Code, Codex, MathML, LaTeXML, XeTeX, pdfTeX, LuaTeX, latexmk, YAML, JSON, ZIP, URL, SSH, HTTPS, MCP, ACP, API, OIDC, ORCID, Okabe-Ito.
- Keyboard key names: Ctrl, Alt, Shift, Enter, Esc, Tab, Cmd, Option, Backspace, Delete, and arrow keys written as in the English string. On macOS-specific strings keep the symbol or key name as in English.
- Command names, file names, extensions and code stay as is: `latexmk`, `.bib`, `main.tex`.
- The Git commands commit, push and pull appear in katakana (コミット, プッシュ, プル) when used as interface words. They stay in Latin letters inside code spans or when the English shows a literal command.
- Commonly kept Latin: AI (write AIエージェント), OK, ID, URL, PDF.
- Do not translate emoji, placeholder names ({count}, {name}) or ICU plural syntax.

## 6. Units and numbers

- Byte units stay in Latin letters with a half-width space: 1.5 MB, 20 GB, 512 KB, 300 B. Use 1024-based units shown with the same labels as the English (KB, MB, GB). Do not write キロバイト or メガバイト in the interface.
- Decimal point is the period (1.5). Thousands separator is the comma (1,024). Do not use Japanese numerals (一二三) for counts. Do not use 万 or 億 grouping.
- Counters: files = {count}個 for items or {count}件 for records and results (changes, comments, matches, errors, projects). People = {count}人. Pages = {count}ページ. Days = {count}日. There is no plural marker.
- Do not put a space between the number and the counter: 「3件」, 「12人」. Between a number and a Latin unit (MB, GB, KB, px) use a half-width space.
- Relative time: 「{days}日前」, 「{hours}時間前」, 「{minutes}分前」, 「たった今」.
- Dates: use year/month/day order with Japanese units in prose (2026年10月31日) or numeric 2026/10/31. Times use 24-hour clock (14:30). Weekday abbreviations in parentheses (土).
- "of" in usage text: 「{used} / {quota} 使用中」 or 「{quota}のうち{used}を使用中」; pick the second in prose.
- Percent: 50% (half-width, no space).
