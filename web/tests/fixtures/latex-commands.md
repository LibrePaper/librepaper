# LaTeX commands and macros to prioritize for a WYSIWYG editor

This is a broad, practical checklist for *text-mode LaTeX* in scholarly manuscripts. **Math commands and mathematical symbols are intentionally excluded** because math is rendered separately with KaTeX. Priorities reflect a suggested implementation order, **not measured frequency**.

- **P0** — core: recognize, render, and support editing.
- **P1** — high: recognize and render; editing should generally work.
- **P2** — medium: recognize and preserve; render if convenient.
- **P3** — long tail: preserve faithfully, possibly using a compiler preview.

Commands listed together usually share an implementation strategy. `environment` names are shown without `\begin`/`\end` unless relevant. Package/class-specific commands are annotated where useful. Starred and optional-argument variants should generally be accepted.

## 1. Structural commands and document metadata

| Priority | Commands / environments | Notes |
|---|---|---|
| P0 | `\documentclass`, `\begin{document}`, `\end{document}` | Document skeleton; class options |
| P0 | `\section`, `\section*`, `\subsection`, `\subsection*`, `\subsubsection`, `\subsubsection*` | Headings; optional short title |
| P0 | `\title`, `\author`, `\date`, `\maketitle` | Title block |
| P0 | `abstract` | Abstract environment |
| P0 | `\input` | External source dependency |
| P0 | `\usepackage` | Package detection / arguments |
| P1 | `\paragraph`, `\paragraph*`, `\subparagraph`, `\subparagraph*` | Run-in headings |
| P1 | `\chapter`, `\chapter*`, `\part`, `\part*` | Reports and books |
| P1 | `\appendix` | Switch section numbering |
| P1 | `\include`, `\includeonly` | Multi-file projects |
| P1 | `\thanks` | Author notes |
| P1 | `\tableofcontents` | Generated navigation |
| P1 | `\keywords`, `\keyword`, `\begin{keywords}` | Class-dependent keywords |
| P1 | `\affiliation`, `\affil`, `\address`, `\email`, `\corref`, `\ead` | Publisher / class-dependent author fields |
| P2 | `\frontmatter`, `\mainmatter`, `\backmatter` | Book structure |
| P2 | `\listoffigures`, `\listoftables` | Generated lists |
| P2 | `\shorttitle`, `\shortauthor`, `\runningtitle`, `\runningauthor` | Class-dependent running headings |
| P2 | `\subtitle`, `\institute` | Class-dependent metadata |
| P2 | `\dedication`, `\acknowledgments`, `acknowledgements` | Class-dependent |
| P3 | `\RequirePackage`, `\PassOptionsToPackage`, `\PassOptionsToClass` | Preamble programming |
| P3 | `\AtBeginDocument`, `\AtEndDocument` | Hooks |

## 2. Inline text formatting

| Priority | Commands | Notes |
|---|---|---|
| P0 | `\textbf`, `\textit`, `\emph`, `\texttt` | Core marks; nested marks |
| P1 | `\textsc`, `\underline`, `\textsuperscript`, `\textsubscript` | Common scholarly formatting |
| P1 | `\textsf`, `\textrm`, `\textnormal` | Font families/reset |
| P1 | `\bfseries`, `\itshape`, `\scshape`, `\ttfamily`, `\rmfamily`, `\sffamily` | Declaration-style formatting; group scope |
| P1 | `\em`, `\normalfont` | Declaration-style emphasis/reset |
| P1 | `\textcolor`, `\color` | `xcolor`; scoped color |
| P2 | `\textsl`, `\textup`, `\textmd` | Font variants |
| P2 | `\mdseries`, `\upshape`, `\slshape` | Font declarations |
| P2 | `\bf`, `\it`, `\rm`, `\tt`, `\sc`, `\sf` | Legacy font switches |
| P2 | `\colorbox`, `\fcolorbox` | `xcolor` backgrounds |
| P2 | `\sout`, `\uline`, `\uuline`, `\uwave`, `\xout` | `ulem` |
| P2 | `\hl`, `\st`, `\ul`, `\so`, `\caps` | `soul` |
| P2 | `\textls`, `\lsstyle` | `microtype` letter spacing |
| P3 | `\fontfamily`, `\fontseries`, `\fontshape`, `\fontencoding`, `\selectfont` | Low-level font selection |
| P3 | `\addfontfeatures` | `fontspec` |

## 3. Inline links, notes, quotations and annotations

| Priority | Commands / environments | Notes |
|---|---|---|
| P0 | `\href`, `\url` | `hyperref`; clickable link |
| P0 | `\footnote` | Inline note with nested content |
| P1 | `\footnotemark`, `\footnotetext` | Split footnotes |
| P1 | `\enquote`, `\enquote*` | `csquotes`; localized quotation |
| P1 | `quote`, `quotation` | Quoted blocks |
| P1 | `\marginpar` | Margin notes |
| P2 | `\nolinkurl`, `\path` | URL/path formatting |
| P2 | `\hyperlink`, `\hypertarget`, `\hyperref` | Internal links |
| P2 | `\textquote`, `\blockquote`, `displayquote`, `displaycquote` | `csquotes` |
| P2 | `\foreignquote`, `\foreigntextquote` | `csquotes` language-aware |
| P2 | `\footnoterule`, `\footnotesep` | Note formatting |
| P2 | `\todo`, `\todo[inline]`, `\missingfigure` | `todonotes` |
| P2 | `\added`, `\deleted`, `\replaced`, `\comment` | `changes` package |
| P3 | `\marginnote`, `\sidenote` | `marginnote` / `sidenotes` |
| P3 | `\endnote`, `\theendnotes` | `endnotes` |
| P3 | `\thanksref`, `\tnoteref`, `\fnref` | Publisher-dependent notes |

## 4. Paragraphs, breaks, alignment, and spacing

| Priority | Commands / syntax | Notes |
|---|---|---|
| P0 | Blank line, `\par` | Paragraph boundaries |
| P0 | `\\`, `\newline` | Manual line breaks |
| P0 | `~`, `\ ` | Nonbreaking/explicit spaces |
| P0 | `%` | Source comments; newline suppression |
| P0 | `\%`, `\&`, `\$`, `\#`, `\_`, `\{`, `\}`, `\textbackslash` | Literal special characters |
| P0 | `--`, `---`, ```` / `''` | Text ligatures / traditional quote markup |
| P1 | `\noindent`, `\indent` | Paragraph indentation |
| P1 | `\centering`, `\raggedright`, `\raggedleft` | Scoped alignment declarations |
| P1 | `center`, `flushleft`, `flushright` | Alignment environments |
| P1 | `\hspace`, `\hspace*`, `\vspace`, `\vspace*` | Explicit spacing |
| P1 | `\quad`, `\qquad`, `\hfill`, `\vfill` | Horizontal / flexible spacing |
| P1 | `\smallskip`, `\medskip`, `\bigskip` | Vertical spacing |
| P1 | `\newpage`, `\clearpage` | Page breaks / float flush |
| P2 | `\pagebreak`, `\nopagebreak`, `\linebreak`, `\nolinebreak` | Page/line break hints |
| P2 | `\allowbreak`, `\-`, `\discretionary` | Hyphenation and discretionary breaks |
| P2 | `\/`, `\@`, `\frenchspacing`, `\nonfrenchspacing` | Text spacing details |
| P2 | `\mbox`, `\makebox`, `\framebox`, `\fbox` | Horizontal boxes |
| P2 | `\parbox`, `minipage` | Block boxes |
| P2 | `\phantom`, `\hphantom`, `\vphantom`, `\smash` | Invisible layout material |
| P2 | `\rule`, `\raisebox` | Positioned layout boxes |
| P3 | `\hskip`, `\vskip`, `\kern`, `\penalty`, `\vadjust` | Low-level TeX layout |
| P3 | `\enlargethispage`, `\samepage`, `\sloppy`, `\fussy` | Line/page fitting |

## 5. Lists

| Priority | Commands / environments | Notes |
|---|---|---|
| P0 | `itemize`, `enumerate`, `\item` | Nested numbered / bulleted lists |
| P1 | `description`, `\item[<label>]` | Labeled lists |
| P1 | `enumerate` / `itemize` optional options | `enumitem` settings such as `label=`, `start=`, `resume`, `nosep` |
| P2 | `\setlist`, `\setlist[itemize]`, `\setlist[enumerate]` | `enumitem` configuration |
| P2 | `\setitemize`, `\setenumerate`, `\setdescription` | Legacy list customization / package-dependent |
| P2 | `\begin{compactitem}`, `\begin{compactenum}` | `paralist` |
| P3 | `inparaenum`, `inparaitem`, `asparaenum` | Inline paragraph lists |
| P3 | `\labelitemi`, `\labelenumi`, `\theenumi` | Customized labels/counters |

## 6. Citations and bibliography

| Priority | Commands / environments | Notes |
|---|---|---|
| P0 | `\cite`, `\citep`, `\citet` | Core citation families; `natbib` for latter two |
| P0 | `\parencite`, `\textcite`, `\autocite` | `biblatex` |
| P0 | `\bibliography`, `\bibliographystyle`, `\addbibresource`, `\printbibliography` | BibTeX / BibLaTeX setup |
| P1 | `\citeauthor`, `\citeyear`, `\citeyearpar`, `\citealt`, `\citealp` | `natbib` |
| P1 | `\footcite`, `\smartcite`, `\supercite`, `\fullcite` | `biblatex` |
| P1 | `\cites`, `\parencites`, `\textcites`, `\autocites` | `biblatex` multicite |
| P1 | `\nocite`, `\bibitem`, `thebibliography` | Uncited/manual bibliography entries |
| P1 | `\citet*`, `\citep*`, `\citeauthor*` | Starred Natbib variants |
| P2 | `\footcitetext`, `\footfullcite`, `\volcite`, `\pvolcite` | `biblatex` |
| P2 | `\citeyear*`, `\citeauthor*`, `\Citep`, `\Citet` | Package-specific capitalization/variants |
| P2 | `\bibpunct`, `\setcitestyle` | `natbib` style |
| P2 | `\printbibliography[heading=...]`, `\defbibheading` | BibLaTeX sectioning |
| P2 | `\newrefsection`, `refsection`, `refsegment` | BibLaTeX partitioning |
| P3 | `\citefield`, `\citelist`, `\citename`, `\citeyearpar` | Bibliography field formatting |
| P3 | `\DeclareFieldFormat`, `\DeclareNameFormat`, `\DeclareBibliographyDriver` | Custom BibLaTeX styles |
| P3 | `\AtEveryBibitem`, `\AtEveryCitekey`, `\ExecuteBibliographyOptions` | BibLaTeX configuration |

Citation parsers should accept multiple keys, one or two optional notes depending on the command, and starred versions. Preserve the source citation form.

## 7. Labels, references, and navigation

| Priority | Commands | Notes |
|---|---|---|
| P0 | `\label`, `\ref`, `\pageref` | Cross-reference definitions/uses |
| P1 | `\autoref`, `\nameref`, `\eqref` | `hyperref` / `amsmath`; `\eqref` is worth recognizing although math is delegated |
| P1 | `\cref`, `\Cref` | `cleveref` |
| P1 | `\crefrange`, `\Crefrange`, `\cpageref` | `cleveref` ranges/pages |
| P2 | `\vref`, `\Vref`, `\vrefrange` | `varioref` |
| P2 | `\namecref`, `\nameCref`, `\labelcref` | `cleveref` |
| P2 | `\hyperref`, `\hyperlink`, `\hypertarget`, `\phantomsection` | Hyperlink anchors |
| P2 | `\hypersetup`, `\urlstyle` | `hyperref` configuration |
| P3 | `\ref*`, `\pageref*`, `\autoref*` | Non-linked variants |
| P3 | `\zlabel`, `\zref` | `zref` package |

## 8. Figures, images, and captions

| Priority | Commands / environments | Notes |
|---|---|---|
| P0 | `figure`, `\includegraphics`, `\caption`, `\label` | Basic figure support |
| P0 | `\includegraphics[width=...,height=...,scale=...]` | Dimension options |
| P1 | `figure*`, `\caption*`, `\graphicspath` | Two-column and nonnumbered figures |
| P1 | `subfigure`, `subtable`, `\subcaptionbox` | `subcaption` |
| P1 | `\captionof`, `\captionof*` | Captions outside floats; `caption` |
| P1 | `\includegraphics[trim=...,clip,angle=...]` | Cropping/rotation |
| P2 | `\subfloat`, `\subref` | `subfig` / legacy subfloat |
| P2 | `\resizebox`, `\scalebox`, `\rotatebox`, `\reflectbox` | `graphicx` transformations |
| P2 | `\ContinuedFloat`, `\captionsetup`, `\subcaptionsetup` | `caption` / `subcaption` |
| P2 | `\FloatBarrier`, `\afterpage` | Float placement helpers |
| P2 | `wrapfigure`, `wraptable`, `sidewaysfigure`, `sidewaystable` | Alternative float layout |
| P2 | `\DeclareGraphicsExtensions` | Image extensions |
| P3 | `\begin{overpic}`, `\put`, `\picture` | Text overlays / picture language |
| P3 | `tikzpicture`, `\tikz`, `\pgfplotsset`, `axis` | Preserve as source or compiler-generated image |
| P3 | `\includepdf`, `\includepdfmerge` | `pdfpages` |

## 9. Tables and tabular text

| Priority | Commands / environments | Notes |
|---|---|---|
| P0 | `table`, `table*`, `tabular`, `&`, `\\` | Floats, cells and rows |
| P0 | `\hline`, `\cline`, `\multicolumn` | Rules and spanning |
| P0 | `\toprule`, `\midrule`, `\bottomrule` | `booktabs` |
| P1 | `tabularx`, `longtable`, `tabular*` | Width / multipage variants |
| P1 | `\cmidrule`, `\addlinespace`, `\specialrule` | `booktabs` refinements |
| P1 | `\multirow`, `\multicolumn` | Cell merges |
| P1 | `\makecell`, `\thead`, `\Xhline`, `\Xcline` | `makecell` |
| P1 | `\arraybackslash`, `\newcolumntype` | `array` extensions |
| P1 | `\num`, `\qty`, `\unit`, `\SI`, `\si` | `siunitx` modern and legacy |
| P1 | `\sisetup`, `S` column type | Number columns |
| P2 | `\rowcolor`, `\cellcolor`, `\columncolor`, `\arrayrulecolor` | `colortbl` |
| P2 | `\tabularnewline`, `\arraystretch`, `\tabcolsep` | Table layout |
| P2 | `\endhead`, `\endfirsthead`, `\endfoot`, `\endlastfoot` | `longtable` sections |
| P2 | `threeparttable`, `tablenotes`, `\tnote` | Caption/table notes |
| P2 | `\tablefootnote`, `\footnotemark`, `\footnotetext` | Table footnotes |
| P2 | `\diagbox`, `\backslashbox` | Diagonal headings |
| P2 | `tabularray`, `tblr`, `longtblr`, `talltblr` | Modern table package |
| P2 | `\SetCell`, `\SetRow`, `\SetColumn`, `\hline`, `\vline` | `tabularray` styling |
| P3 | `xtabular`, `supertabular`, `ltablex`, `xltabular` | Other long/wide tables |
| P3 | `\extracolsep`, `\doublerulesep`, `\arrayrulewidth` | Low-level layout |
| P3 | `\rowcolors` | Alternating colored rows |

Column specifications deserve their own parsing rules: `l`, `c`, `r`, `p{}`, `m{}`, `b{}`, `X`, `S`, `|`, `@{}`, `!{}`, `>{}`, `<{}`, and `*{n}{spec}`. Package-specific extensions may add more.

## 10. Code and verbatim material

| Priority | Commands / environments | Notes |
|---|---|---|
| P0 | `\verb`, `verbatim`, `verbatim*` | Special lexical handling |
| P1 | `\lstinline`, `lstlisting`, `\lstinputlisting` | `listings` |
| P1 | `minted`, `\mintinline`, `\inputminted` | `minted` |
| P1 | `\lstset`, `\lstdefinestyle` | Listings configuration |
| P2 | `\Verbatim`, `Verbatim`, `BVerbatim`, `\Verb` | `fancyvrb` |
| P2 | `\DefineVerbatimEnvironment`, `\SaveVerb`, `\UseVerb` | `fancyvrb` |
| P2 | `\newminted`, `\setminted`, `\usemintedstyle` | `minted` configuration |
| P2 | `algorithm`, `algorithmic`, `algorithm2e` | Algorithm blocks |
| P2 | `\State`, `\Statex`, `\If`, `\ElsIf`, `\Else`, `\EndIf` | `algpseudocode` |
| P2 | `\For`, `\ForAll`, `\While`, `\EndFor`, `\EndWhile` | `algpseudocode` |
| P2 | `\Function`, `\Procedure`, `\Return`, `\Require`, `\Ensure` | `algpseudocode` |
| P3 | `\KwData`, `\KwResult`, `\KwIn`, `\KwOut`, `\tcp`, `\tcc` | `algorithm2e` |
| P3 | `alltt`, `comment`, `lstlisting*` | Special block variants |

## 11. Theorem-like and scholarly environments

| Priority | Commands / environments | Notes |
|---|---|---|
| P1 | `\newtheorem`, `theorem`, `lemma`, `proposition`, `corollary` | User-defined environment names |
| P1 | `definition`, `proof`, `assumption` | Common formal blocks |
| P2 | `remark`, `example`, `claim`, `conjecture`, `axiom`, `hypothesis` | More theorem-like blocks |
| P2 | `\theoremstyle`, `\newtheoremstyle` | `amsthm` |
| P2 | `\qed`, `\qedhere`, `\pushQED`, `\popQED` | `amsthm` proof |
| P2 | `\numberwithin`, `\counterwithin` | Numbering configuration |
| P2 | `\newmdtheoremenv`, `mdframed` | `mdframed` extensions |
| P3 | `tcolorbox`, `\newtcolorbox`, `\newtcbtheorem` | Styled theorem/callout boxes |

## 12. Text symbols, punctuation and accents (non-math)

| Priority | Commands / syntax | Notes |
|---|---|---|
| P0 | `\%`, `\&`, `\$`, `\#`, `\_`, `\{`, `\}`, `\textbackslash` | Escaped specials |
| P0 | `\ldots`, `\dots`, `\textellipsis` | Ellipsis |
| P1 | `\textemdash`, `\textendash`, `\textquotedblleft`, `\textquotedblright` | Dashes/quotes |
| P1 | `\textquoteleft`, `\textquoteright`, `\textquotesingle` | Quotes/apostrophes |
| P1 | `\LaTeX`, `\TeX`, `\LaTeXe` | Typeset names |
| P1 | `\copyright`, `\textcopyright`, `\textregistered`, `\texttrademark` | Legal marks |
| P1 | `\textdegree`, `\textmu`, `\textnumero` | Common text symbols |
| P1 | `\oe`, `\OE`, `\ae`, `\AE`, `\ss`, `\SS` | Letter forms |
| P1 | `\o`, `\O`, `\aa`, `\AA`, `\l`, `\L` | Letter forms |
| P1 | `\'`, `\``, `\^`, `\"`, `\~`, `\c`, `\v` | Accents; attached arguments |
| P2 | `\H`, `\u`, `\=`, `\.`, `\b`, `\d`, `\r`, `\k`, `\t` | More accents |
| P2 | `\i`, `\j` | Dotless letters for accents |
| P2 | `\textbullet`, `\textperiodcentered`, `\textasteriskcentered` | Text punctuation |
| P2 | `\textdagger`, `\textdaggerdbl`, `\textsection`, `\textparagraph` | Editorial symbols |
| P2 | `\pounds`, `\textsterling`, `\texteuro`, `\textyen`, `\textdollar` | Currency symbols |
| P2 | `\textasciitilde`, `\textasciicircum`, `\textless`, `\textgreater` | Literal ASCII-like glyphs |
| P2 | `\textbar`, `\textbrokenbar`, `\textbraceleft`, `\textbraceright` | Additional delimiters as text |
| P2 | `\textordfeminine`, `\textordmasculine`, `\textonehalf`, `\textonequarter` | Text-comp symbols |
| P2 | `\og`, `\fg`, `\guillemotleft`, `\guillemotright` | French guillemets; package-dependent |
| P3 | `\textflorin`, `\textcurrency`, `\textperthousand` | Less common text symbols |
| P3 | `\ding` | `pifont`; glyph by number |
| P3 | `\faIcon`, `\faGithub`, `\faEnvelope` | `fontawesome5`, icon macros |

## 13. Language, localization, and glossary/acronym commands

| Priority | Commands / environments | Notes |
|---|---|---|
| P1 | `\selectlanguage`, `\foreignlanguage`, `otherlanguage` | `babel` |
| P1 | `\enquote`, `\enquote*` | Locale-aware punctuation |
| P2 | `\setmainlanguage`, `\setotherlanguage`, `\text<language>` | `polyglossia`; language-specific macros vary |
| P2 | `\babelprovide`, `\babeltags` | Modern `babel` |
| P2 | `\gls`, `\Gls`, `\glspl`, `\Glspl` | `glossaries` |
| P2 | `\ac`, `\acs`, `\acl`, `\acf`, `\acp` | `acronym` |
| P2 | `\glsfirst`, `\glsentrytext`, `\glsentrylong` | Glossary variants |
| P2 | `\newacronym`, `\newglossaryentry`, `\printglossaries`, `\printglossary` | Definitions/output |
| P2 | `\DeclareAcronym`, `\ac`, `\acs`, `\acl`, `\acf` | `acro` package; semantics differ |
| P3 | `\setacronymstyle`, `\glsadd`, `\glsaddall`, `\glsresetall` | Glossary state |
| P3 | `\MakeUppercase`, `\MakeLowercase`, `\uppercase`, `\lowercase` | Case conversions |

## 14. Macro declarations and programmable LaTeX

| Priority | Commands | Notes |
|---|---|---|
| P0 | `\newcommand`, `\renewcommand`, `\providecommand` | Common custom macros; optional/default arguments |
| P0 | `\newenvironment`, `\renewenvironment` | Custom environments |
| P1 | `\DeclareRobustCommand`, `\NewDocumentCommand`, `\RenewDocumentCommand`, `\ProvideDocumentCommand` | Command definitions |
| P1 | `\NewDocumentEnvironment`, `\RenewDocumentEnvironment` | Modern environment definitions |
| P1 | `\def`, `\let` | TeX macro definitions/aliases |
| P1 | `\DeclareMathOperator` | Recognize math macro definitions even though KaTeX handles math display |
| P1 | `\newcolumntype` | Table macro definitions |
| P2 | `\gdef`, `\edef`, `\xdef` | Expanded/global TeX definitions |
| P2 | `\newlength`, `\setlength`, `\addtolength` | Length variables |
| P2 | `\newcounter`, `\setcounter`, `\addtocounter`, `\stepcounter`, `\refstepcounter` | Counters |
| P2 | `\newif`, `\iftrue`, `\iffalse`, `\else`, `\fi` | Conditions |
| P2 | `\ifdefined`, `\ifx`, `\ifnum`, `\ifdim` | Low-level conditions |
| P2 | `\makeatletter`, `\makeatother` | Internal `@` control sequence names |
| P2 | `\DeclareOption`, `\ProcessOptions` | Package/class options |
| P3 | `\csname`, `\endcsname`, `\expandafter`, `\noexpand`, `\protect` | TeX expansion primitives |
| P3 | `\global`, `\long`, `\outer`, `\futurelet` | TeX definitions |
| P3 | `\catcode` | Changes lexing; preserve or isolate |
| P3 | `\ExplSyntaxOn`, `\ExplSyntaxOff` | LaTeX3 parsing mode |
| P3 | `\cs_new:Npn`, `\tl_set:Nn`, `\seq_new:N`, `\prop_new:N` | Representative LaTeX3 programming; opaque fallback |
| P3 | `\NewExpandableDocumentCommand`, `\DeclareDocumentCommand` | Advanced `xparse`/kernel |

## 15. Page layout and typography

| Priority | Commands / environments | Notes |
|---|---|---|
| P1 | `\small`, `\footnotesize`, `\normalsize`, `\large`, `\Large`, `\LARGE` | Scoped size declarations |
| P1 | `\tiny`, `\scriptsize`, `\huge`, `\Huge` | More sizes |
| P1 | `\onehalfspacing`, `\doublespacing`, `\singlespacing`, `spacing` | `setspace` |
| P1 | `\geometry`, `\newgeometry`, `\restoregeometry` | `geometry` |
| P2 | `\setlength{\parindent}{...}`, `\setlength{\parskip}{...}` | Paragraph layout |
| P2 | `\setlength{\textwidth}{...}`, `\setlength{\textheight}{...}` | Page metrics |
| P2 | `\linespread`, `\fontsize`, `\selectfont` | Typography |
| P2 | `\pagestyle`, `\thispagestyle`, `\pagenumbering`, `\setcounter{page}{...}` | Headers/footers/pagination |
| P2 | `\twocolumn`, `\onecolumn`, `multicols`, `\columnbreak` | Multi-column layouts |
| P2 | `\fancyhead`, `\fancyfoot`, `\fancyhf`, `\headrulewidth` | `fancyhdr` |
| P2 | `\titleformat`, `\titlespacing` | `titlesec` |
| P2 | `\setmainfont`, `\setsansfont`, `\setmonofont` | `fontspec` |
| P2 | `\microtypesetup` | `microtype` |
| P2 | `\raggedbottom`, `\flushbottom` | Page vertical layout |
| P3 | `\topmargin`, `\oddsidemargin`, `\evensidemargin`, `\headheight`, `\footskip` | Low-level page dimensions |
| P3 | `\widowpenalty`, `\clubpenalty`, `\interlinepenalty`, `\tolerance`, `\emergencystretch` | Break optimization |
| P3 | `\enlargethispage`, `\enlargethispage*` | Page tweaking |

## 16. Common supplementary packages and miscellaneous macros

| Priority | Commands / environments | Notes |
|---|---|---|
| P1 | `\SI`, `\si`, `\num`, `\qty`, `\unit`, `\ang`, `\sisetup` | `siunitx` text/unit handling |
| P2 | `\ac`, `\gls`, `\newacronym` | Acronyms/glossary |
| P2 | `\epigraph`, `\epigraphhead` | `epigraph` |
| P2 | `\lettrine` | Drop caps |
| P2 | `\blindtext`, `\lipsum` | Placeholder content |
| P2 | `\includecomment`, `\excludecomment` | `comment` package |
| P2 | `\pdfbookmark`, `\bookmark`, `\bookmarksetup` | PDF outline/navigation |
| P2 | `\includepdf` | Embedded PDF pages |
| P3 | `\printindex`, `\index`, `\makeindex` | Indexing |
| P3 | `\nomenclature`, `\printnomenclature` | Nomenclature |
| P3 | `\newtcolorbox`, `\tcbset`, `tcolorbox` | Box/callout styling |
| P3 | `\begin{mdframed}`, `\mdfsetup` | Framed content |
| P3 | `\begin{landscape}`, `\begin{adjustwidth}` | Page orientation / indents |
| P3 | `\begin{sideways}`, `\begin{turn}` | Rotating package |
| P3 | `\includestandalone`, `\standaloneconfig` | Standalone figures |
| P3 | `\externaldocument` | Cross-file references via `xr`/`xr-hyper` |

## 17. Syntax and constructs that are **not commands** but must be recognized

These matter at least as much as any individual macro.

| Priority | Syntax | Notes |
|---|---|---|
| P0 | `\command{arg}` | Mandatory braced argument |
| P0 | `\command[opts]{arg}` | Optional argument; nested brackets may need careful handling |
| P0 | `\command*{arg}` | Starred variants |
| P0 | `{ ... }` | Groups and scope |
| P0 | `\begin{name} ... \end{name}` | Nested environments |
| P0 | `% ... newline` | Comments and newline handling |
| P0 | `\input{file}`, `\include{file}` | Project files / import graph |
| P0 | `\verb|...|`, `verbatim`, `lstlisting`, `minted` | Distinct lexing modes |
| P0 | `$...$`, `\(...\)`, `\[...\]`, `$$...$$` | Recognize boundaries only; hand off to KaTeX |
| P0 | `&`, `\\` within table-like environments | Structural separators, context-dependent |
| P1 | `[htbp!]`, `[H]` | Float placement parameters |
| P1 | `\includegraphics[key=value,...]{path}` | Graphic options / file dependencies |
| P1 | `\citep[pre][post]{key1,key2}` | Citation arguments vary by family |
| P1 | `\newcommand{\foo}[2][default]{...}` | Definition signature and arguments |
| P1 | `\newenvironment{name}[n]{before}{after}` | Environment definition |
| P1 | `\NewDocumentCommand{\foo}{O{default} m}{...}` | xparse signatures |
| P1 | `p{width}`, `m{width}`, `X`, `S`, `@{...}`, `>{...}`, `*{n}{...}` | Table column grammar |
| P2 | `\if... \else ... \fi` | Conditional branches; opaque if unevaluated |
| P2 | `\ExplSyntaxOn ... \ExplSyntaxOff` | Different control sequence character conventions |
| P3 | `\catcode` changes | Fundamentally changes lexical rules |

## Suggested interpretation of priorities

The highest-value implementation areas for journal articles are **headings; text styles; links; footnotes; paragraphs and lists; citations; labels/references; images; captions; tables; bibliography integration; and custom macros**. Everything else should be parsed enough to survive a round-trip even when the editor cannot display it structurally.

Keep three capabilities separate in your implementation: **recognition**, **rendering**, and **structural editing**. A P3 construct can still have excellent source preservation without implementing its full behavior. For complicated custom TeX, preserve exact source and optionally display a compiler-produced preview rather than attempting to emulate TeX itself.
