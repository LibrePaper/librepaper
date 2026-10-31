= Results <chap-results>

This chapter reports the simulation results. The estimates in @tab-book-results come from 500 replications, and each row follows the design in #ref(<chap-methods>, supplement: [Chapter]).

== Simulation summary <sec-simulation>

#figure(
  table(
    columns: (1fr, auto, auto),
    align: (left, right, right),
    stroke: none,
    table.hline(),
    table.header[*Estimator*][*Estimate*][*Coverage*],
    table.hline(),
    [Difference in means], [0.52 (0.11)], [0.94],
    [Regression adjustment], [0.49 (0.08)], [0.95],
    [Bootstrap percentile], [0.50 (0.09)], [0.93],
    table.hline(),
  ),
  caption: [Estimated treatment effect by estimator. Standard errors in parentheses.],
) <tab-book-results>

Regression adjustment has the smallest standard error. The bootstrap intervals cover close to the nominal rate, which supports the check described in @eq-bootstrap-se.

== Interpretation <sec-interpretation>

The three estimators agree on the size of the effect. The choice between them comes down to precision and the cost of the extra modeling step, as Rubin #cite(<rubin1974>, form: "prose") anticipated for randomized designs. #ref(<chap-introduction>, supplement: [Chapter]) frames why these trade-offs matter for the questions the book opens with.
