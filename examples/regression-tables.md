# What a Regression Table Is Hiding

A regression table is a summary, and every summary is a decision about what to
leave out. The trouble is that the omissions are conventional rather than
deliberate, so a reader learns to skim past exactly the places where the
argument is weakest.

This note is a checklist. It is written in Markdown and Komodoc rendered it to
HTML when it was published: no toolchain ran over it first.

## Start at the bottom

Most readers begin at the coefficient of interest and stop there. Begin instead
with the three numbers underneath it.

- **N.** How many observations, and how many were dropped to get there? A model
  fit on 4,102 of 11,000 rows is a model of the 4,102.
- **The unit.** One row per person, per person-year, or per person-month
  changes the standard errors by more than most of the specification choices
  above it.
- **What is being clustered on.** Standard errors clustered at the wrong level
  are not conservative; they are simply wrong, and usually too small.

## Stars are not evidence

The convention of marking `p < 0.05` with an asterisk does two things at once.
It compresses a continuous quantity into a binary one, and it puts the
threshold where the reader cannot see it.

| Reported | What it rules out | What it does not |
| --- | --- | --- |
| `p < 0.05` | Chance alone, under the model | That the model is right |
| A wide interval | Very little | Being confused with precision |
| A tight interval | A large effect | Bias of any size |

A tight interval around a biased estimate is the most misleading object in
applied statistics, because it looks exactly like a good result.

## The specification you were not shown

Every table is one column of a much wider table that was never printed. The
question is not whether the authors explored alternatives — they did — but
whether the printed column is representative of what they found.

1. Are the controls the ones a reader would have chosen in advance?
2. Does the coefficient survive dropping any single control?
3. If the sample is split, does the sign hold in both halves?

The third is the one worth asking out loud. An effect that appears in the
pooled data and in neither half is not a subtle effect. It is an artefact of
pooling.

## On rounding

Report two significant figures for the estimate and the same for its
uncertainty, and do not let the software choose. Four decimal places on a
coefficient whose standard error is 0.3 is not precision; it is noise, printed.

## What a good table admits

The best tables say what they cannot show. A footnote reading *the instrument
is weak in the second stage; see Table A4* has done more for the reader than
another row of stars ever will. A table that admits nothing is not a table
without problems.
