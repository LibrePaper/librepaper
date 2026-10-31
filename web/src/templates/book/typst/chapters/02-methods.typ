= Methods <chap-methods>

This chapter describes the estimators and the assumptions behind them. The notation stays the same throughout the book.

== Estimators <sec-estimators>

The bootstrap estimates the sampling distribution of a statistic by resampling the data with replacement @efron1979. For a statistic $hat(theta)$ computed on $B$ resamples, the bootstrap estimate of its standard error is

$ hat(s)e = sqrt(1 / (B - 1) sum_(b=1)^B (hat(theta)_b^* - macron(theta)^*)^2), quad macron(theta)^* = 1 / B sum_(b=1)^B hat(theta)_b^* $ <eq-bootstrap-se>

@eq-bootstrap-se needs no distributional assumption, which is why it is the default check in this book.

== Assumptions <sec-assumptions>

Each estimator relies on the same design. Units are randomized to treatment, and the outcome follows the linear model introduced in @sec-model-notation. Violations of randomization are outside the scope of this chapter, and #ref(<chap-introduction>, supplement: [Chapter]) points to the literature that covers them.

== Model notation <sec-model-notation>

Let $Y_i$ be the outcome and $D_i in {0, 1}$ the treatment for unit $i$. The statistic of interest is the difference in means between the two arms, which estimates the treatment effect $beta_1$.
