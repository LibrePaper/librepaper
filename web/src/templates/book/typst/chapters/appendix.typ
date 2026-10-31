#heading(level: 1, outlined: true)[Supplementary derivation] <app-derivation>

This appendix derives the variance of the difference-in-means estimator used in #ref(<chap-results>, supplement: [Chapter]). The derivation assumes the design in @sec-assumptions.

Under randomization, the two arms have sample sizes $n_1$ and $n_0$, and the difference in means has variance

$ op("Var")(hat(beta)_1) =sigma_1^2 / n_1 + sigma_0^2 / n_0 $ <eq-dim-variance>

where $sigma_d^2$ is the outcome variance in arm $d in {0, 1}$. Taking square roots of @eq-dim-variance gives the standard error reported in the main text.
