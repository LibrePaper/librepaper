= What a Confidence Interval Does Not Say

A confidence interval is a statement about a procedure, not about a parameter. The distinction sounds pedantic until you watch what readers do with the number, at which point it becomes the whole difference between a result and a claim.

This note is written in Typst, and LibrePaper rendered it to HTML when it was published: no toolchain ran over it first. The same compiler runs in the editor, so the preview beside the source is the document you are reading.

== The procedure, not the parameter

The interval

$ macron(x) plus.minus t_(alpha\/2, n-1) dot s / sqrt(n) $

covers the true mean in 95 out of every 100 samples, over repetitions that never happened and mostly never will. It does not say there is a 95% chance the mean lies inside this one. The parameter is fixed; the interval is what moved.

Nothing goes wrong until someone writes "we are 95% confident the effect is between 0.2 and 0.8" and a reader hears a probability about the effect. That reader is not being careless. The sentence invites the reading.

== Three habits worth keeping

- *Report the width before the endpoints.* An interval of $plus.minus 0.03$ and an interval of $plus.minus 0.9$ are different studies, and the endpoints disguise which one you ran.
- *Say what varies.* Sampling error is one source of uncertainty and rarely the largest. An interval that accounts only for it is a lower bound on how wrong you might be.
- *Resist the ceremony of exclusion.* Whether the interval excludes zero is a fact about the interval, not a finding. It is the same significance test wearing a different coat.

== What the width is made of

For a difference in means with equal variances,

$ "SE" = s_p sqrt(1/n_1 + 1/n_2) $

which halves only when the sample quadruples. Precision is expensive, and it is worth saying out loud how much precision the design could ever have bought. A study powered to detect an effect of 0.5 will produce intervals about that wide whatever the truth is, and reporting one as though its endpoints were measurements overstates what the design could resolve.

== A closing note

The interval is honest arithmetic about a procedure you chose. Most of the uncertainty in an empirical paper is in the choosing, and no interval has ever covered that.
