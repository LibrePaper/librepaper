---
title: "Getting the review out"
---

## Export annotations

A document's annotations come out as readable Markdown, or as W3C Web
Annotation JSON-LD for anything that wants to process them. Both are exports
from the terminal; see [Export](../cli.html#export).

## Response to reviewers

The response can be limited to the comments made since a given checkpoint, covering only what is new.


```markdown
## Reviewer: annegrandchamp

### 1. commenting, resolved in d1e0f42

> The confidence interval does not say that the parameter is inside it with 95% probability.

**Then:** "with 95% probability, the true value lies in the interval"

**Now:** no longer in the document.

**Vincent:** Fixed as suggested; see also the new footnote on coverage.
```

Use `--since` to keep only comments made at or after a checkpoint. **Then** is a quotation rather than a recollection, because every comment records the checkpoint it was made on. **Now** says whether the passage is still in the document and quotes its replacement when the word diff can identify it. The line is omitted for documents this machine cannot render, such as LaTeX papers.
