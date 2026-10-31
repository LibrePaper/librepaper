// A research talk with the Mosaic package and its Metropolis theme.
// Edit the title block, then replace the slides with your own content.
#import "@preview/mosaic:0.0.1" as m

#show: m.themes.metropolis.setup.with(
  title: "{{title}}",
  subtitle: [A short research talk],
  authors: "{{author}}",
  date: datetime.today().display(),
  font: "Libertinus Serif",
  font-mono: "DejaVu Sans Mono",
)

#set math.equation(numbering: "(1)")

#m.slide(layout: "title")

= Motivation

== Why this question matters

Survey response rates have fallen for two decades.

#m.steps.pause

Low response makes estimates sensitive to who answers.

#m.steps.pause

Existing corrections rest on strong assumptions.

== What we do

- Combine survey data with administrative records.
- Reweight the survey to match known population totals.
- Bound the effect under weaker assumptions.

= Data and design

#m.slide(
  layout: m.layouts.content(variant: "header-body", columns: 2),
  [== Two data sources],
  [
    *Administrative records*

    Complete for every household, but coarse on income.
  ],
  [
    *Survey panel*

    Rich covariates, but response is selective.
  ],
)

== Estimator

We estimate the coefficient by least squares:

$ hat(beta) = (X^top X)^(-1) X^top y $ <eq-ols>

@eq-ols holds under the classical assumptions.

== Three kinds of result

#m.components.card(role: "accent")[
  *Main finding.* The effect survives reweighting.
]

#m.components.callout(role: "warning", title: [Caveat])[
  Bounds widen when the response model is misspecified.
]

= Results

== Estimated effects

#m.note[Mention the bounds before the table.]

#table(
  columns: (2fr, 1fr, 1fr),
  align: (left, center, center),
  stroke: none,
  table.hline(stroke: 0.8pt),
  table.header[*Specification*][*Estimate*][*Std. error*],
  table.hline(stroke: 0.4pt),
  [Baseline], [0.042], [0.011],
  [Reweighted], [0.037], [0.013],
  [Bounded], [0.029], [0.015],
  table.hline(stroke: 0.8pt),
)

== Robustness

The sign pattern holds across all three specifications.

#m.steps.pause

Magnitudes shrink once the bounds are imposed.

= Conclusion

== Takeaways

- The main estimate is stable under reweighting.
- Weaker assumptions widen the interval but keep its sign.
- Response data should come with a sensitivity analysis.

#m.slide(layout: "section", invert: true)[= Questions?]

= Appendix

== Backup: alternative weights

Inverse probability weights give the same sign pattern.
The standard errors are slightly larger.
