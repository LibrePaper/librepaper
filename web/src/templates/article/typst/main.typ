#import "@preview/arkheion:0.1.2": arkheion

// Edit the title, author and affiliation below
#show: arkheion.with(
  title: "{{title}}",
  authors: (
    (name: "{{author}}", email: "author@example.org", affiliation: "Department of Statistics, University of Example"),
  ),
  abstract: [
    We study how a simple linear model recovers a treatment effect from a randomized experiment. We compare three estimators in a small simulation and report their bias and coverage. The results favour regression adjustment when the outcome has a strong predictor, and the bootstrap gives reliable intervals in both settings.
  ],
  keywords: ("causal inference", "regression adjustment", "bootstrap", "simulation"),
  date: datetime.today().display("[month repr:long] [day padding:none], [year]"),
)

#set cite(style: "chicago-author-date")

= Introduction <sec-introduction>

Information theory @shannon1948 and resampling @efron1979 remain the workhorses of statistical inference. This paper asks how much a simple model can recover from a randomized experiment, and how much an analyst should trust the resulting intervals.

Work on the potential outcomes framework #cite(<rubin1974>, form: "prose") and on ensemble methods @breiman2001 motivates the design of the study. Our contributions are threefold. First, we state the identification result in @sec-model. Second, we compare three estimators in @sec-results. Third, we discuss when each estimator should be preferred in @sec-conclusion.

= Related work <sec-related>

Early treatments of experimental design focus on randomization as the source of identification. Modern practice adds covariates to gain precision, a step that is now standard in applied work @rubin1974. Resampling methods provide intervals without distributional assumptions @efron1979, and they have been studied extensively for ensembles of trees @breiman2001.

= Model <sec-model>

Let $Y_i$ denote the outcome for unit $i$ and let $D_i in {0, 1}$ indicate treatment. We assume the linear model
$ Y_i = beta_0 + beta_1 D_i + epsilon.alt_i $ <eq-model>
with $bb(E)[epsilon.alt_i | D_i] = 0$. Under @eq-model, the difference in means recovers the treatment effect, as stated in the following result.

#block(inset: (left: 1em), stroke: (left: 1.5pt + gray))[
  *Theorem 1* (Identification). Suppose @eq-model holds and treatment is randomly assigned, so that $epsilon.alt_i$ is independent of $D_i$. Then $beta_1 = bb(E)[Y_i | D_i = 1] - bb(E)[Y_i | D_i = 0]$.
]

_Proof._ Take conditional expectations of @eq-model given $D_i$. The error term has mean zero in each arm, so the difference in the two conditional means equals $beta_1$. #h(1fr) $square$

= Results <sec-results>

@tab-results reports the estimates from a simulation with 500 replications. Each cell gives the mean estimate with its standard error in parentheses, and the coverage of the nominal 95% interval.

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
) <tab-results>

Regression adjustment has the smallest standard error, as expected from the variance reduction it provides. The bootstrap intervals are slightly narrower than the normal approximation and still cover close to the nominal rate.

= Conclusion <sec-conclusion>

Under randomization, a linear model gives a transparent route to the treatment effect. Regression adjustment improves precision at little cost, and the bootstrap is a sound check when the distribution of the estimator is unclear. Future work should extend the comparison to clustered designs.

#bibliography("references.bib", style: "chicago-author-date")
