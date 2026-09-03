#set document(title: [A Short Style Guide for Quantitative Writing])

= A Short Style Guide for Quantitative Writing

This is plain Typst, compiled with `typst compile --format html --features html`
and nothing else: no packages, no executed code, no preprocessing. Headings,
lists and tables, which is most of what a document needs.

== Numbers in prose

A number in a sentence is being read, not computed. Round it until it can be
held in the head.

- Two significant figures in the text, full precision in the table.
- Units every time, without exception.
- A percentage change and a percentage-point change are different quantities,
  and swapping them silently is the commonest error in this genre.

+ State the estimate.
+ State its uncertainty.
+ State what would have to be true for it to be wrong.

== Choosing an interval

The three intervals below answer three different questions, and the choice is
substantive rather than stylistic.

#table(
  columns: (auto, auto, auto),
  align: (left, left, left),
  table.header([*Interval*], [*Answers*], [*Fails when*]),
  [Confidence], [Where is the parameter, across repeated samples?], [The model is wrong],
  [Prediction], [Where will the next observation fall?], [The variance is misjudged],
  [Tolerance], [Where do most of the population lie?], [The tails are heavier than assumed],
)

A confidence interval for a mean is often reported where a prediction interval
was wanted. The first is narrow and about a parameter; the second is wide and
about an observation.

== Tables

A table is read down its columns, so put the comparison in the columns and the
cases in the rows.

#table(
  columns: (auto, auto, auto, auto),
  align: (left, right, right, right),
  table.header([*Method*], [*Estimate*], [*Std. error*], [*Coverage*]),
  [Normal approximation], [4.02], [0.31], [0.91],
  [Percentile bootstrap], [4.02], [0.33], [0.93],
  [BCa bootstrap], [4.03], [0.33], [0.95],
  [Exact], [4.00], [0.32], [0.95],
)

Three rules that survive most contact with reality:

- Right-align numbers, left-align text, and align on the decimal point where the
  format allows it.
- Give every column a unit or a scale in its header, not in a footnote.
- Sort rows by something meaningful. Alphabetical order is meaningful only for
  looking things up.

== Figures against tables

Use a figure when the shape of the relationship is the finding. Use a table when
the individual values are the finding. A figure that could have been a sentence
should be a sentence.

== Words to avoid

#table(
  columns: (auto, auto),
  align: (left, left),
  table.header([*Instead of*], [*Write*]),
  [significant], [large, or reliable, or _p_ < 0.05, whichever you mean],
  [correlated with], [associated with, or predicts, or causes, whichever you mean],
  [proves], [is consistent with],
  [very], [nothing at all],
)

"Significant" is the worst of these, because it has a technical meaning and an
ordinary one, and a reader cannot tell from the sentence which was intended.

== A closing note

The purpose of all of this is to make the reader's job easy, not to make the
writer's job look hard. If a sentence needs a second reading to yield its
meaning, and that second reading was not the point, rewrite it.
