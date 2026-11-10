---
title: "Edge cases"
author: "A. Author"
bibliography: refs.bib
format:
  html:
    toc: true
---

<!-- an html comment with *emphasis* and $math$ -->

# Heading one {#sec-one}

Setext heading
==============

Second setext
-------------

### Heading three ###

Plain *emphasis*, _underscore emphasis_, **strong**, __strong too__, ***both***, ~~strike~~, and `inline code`.
Escapes: \* star, \_ under, \# hash, \$ dollar, \@ at, \\ slash.

Inline math $x^2 + y_i$ and $$\int_0^1 f(x)\,dx$$ display.

$$
\sum_{k=1}^{n} k = \frac{n(n+1)}{2}
$$

```python
#| label: fig-code
#| fig-cap: "A caption"
#| echo: false
print("hi")
```

```{r}
#| warning: false
x <- 1:10
summary(x)
```

```
plain fence no language
```

::: {.callout-note title="Heads up"}
A callout with **markup** inside.
:::

:::: {.columns}
::: {.column width="50%"}
Left column.
:::
::: {.column width="50%"}
Right column.
:::
::::

::: {#thm-pythag .theorem}
Right triangles satisfy $a^2 + b^2 = c^2$.
:::

| Name  | Value | Note    |
|:------|------:|:-------:|
| alpha |     1 | first   |
| beta  |     2 | `code`  |

Table: A caption for the table. {#tbl-vals}

> A block quote
> spanning lines
>
> > nested quote

- Bullet one
- Bullet two
    - Nested bullet
        - Deeper

1. Ordered one
2. Ordered two
   1. Nested ordered

- [ ] Open task
- [x] Done task
- [/] Doing task #priority:high @due(2026-12-01)
- [-] Dropped task

Footnote reference here[^1] and another[^note].

[^1]: The first footnote.
[^note]: A longer note
    with a continuation line.

[link text](https://example.org/path "Title") and [ref link][ref] and [ref].

[ref]: https://example.org/ref  "Ref title"
[ref2]: <https://example.org/angle>

Citations: [@a; @b], [see @a, p. 3], @a says so, and @doe2019 [-@roe2021] suppressed.

Images: ![Alt text](img/pic.png){#fig-pic width=80% fig-align="center"}
and ![](img/bare.png).

Shortcodes: {{< video https://example.org/v.mp4 >}} and {{< meta title >}}.

Autolink: <https://example.org/auto> and bare https://example.org/bare, plus www.example.com.

Emoji and a line break at the end\
next line.

***

___

Final paragraph with a [^1] reference and an unclosed *emphasis and `unclosed code.
