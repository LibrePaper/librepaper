---
title: "Getting the review out"
---

## Export annotations


Export annotations as readable Markdown. `export` takes the short ID from
`list` (a full slug also works):

```sh
librepaper export c9k --format markdown --output comments.md
```

Without `--format markdown`, LibrePaper exports W3C Web Annotation JSON-LD.

## Response to reviewers


The one export that is not a list of what was said. `--format response` writes
the document an author has to produce anyway: grouped by reviewer, numbered
within each, with the remark, the passage as that reviewer saw it, what became
of it since, and the thread underneath as the answer.

```sh
librepaper export c9k --format response --since 4f2a91c --output response.md
```

```markdown
## Reviewer: annegrandchamp

### 1. commenting, resolved in d1e0f42

> The confidence interval does not say that the parameter is inside it with 95% probability.

**Then:** “with 95% probability, the true value lies in the interval”

**Now:** no longer in the document.

**Vincent:** Fixed as suggested; see also the new footnote on coverage.
```

Replying to a comment in the reader is writing this document. `--since` takes
a checkpoint selected in the browser's History panel and keeps the comments
made at or after it, which is a round of review.

**Then** is a quotation rather than a recollection, because every comment
records the checkpoint it was made on. **Now** says whether the passage is
still in the document and quotes its replacement when the word diff can
identify it. The line is left out entirely for a
document this machine cannot render -- a LaTeX paper, whose compiler is in a
browser.
