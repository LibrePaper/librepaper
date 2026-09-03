#import "/.calepin/calepin.typ" as calepin
#show: calepin.document

#set document(title: [How Far Does a Drunk Walk?])
#calepin.setup(echo: true, eval: true, results: "verbatim", fenced-chunks: true)

#title()

A random walk takes steps of $plus.minus 1$, each direction equally likely, each
independent of the last. After $n$ steps its position is

$ S_n = sum_(i=1)^n X_i, quad X_i = cases(+1 "with probability" 1\/2, -1 "with probability" 1\/2) $

The mean is zero by symmetry. That fact is true and almost useless, because it
describes where the walk is on average, not how far from home it tends to be.

= The right question

The quantity that carries the information is the variance:

$ "Var"(S_n) = sum_(i=1)^n "Var"(X_i) = n $

so the typical distance from the origin grows like $sqrt(n)$, not like $n$. Four
times as many steps take you only twice as far.

```r
set.seed(20260903)

walk <- function(n) cumsum(sample(c(-1, 1), n, replace = TRUE))
distance <- function(n, reps = 2000) mean(abs(replicate(reps, walk(n)[n])))

steps <- c(25, 100, 400, 1600, 6400)
data.frame(
  n         = steps,
  mean_dist = round(vapply(steps, distance, numeric(1)), 2),
  sqrt_n    = round(sqrt(steps), 2),
  ratio     = round(vapply(steps, distance, numeric(1)) / sqrt(steps), 3)
)
```

The ratio settles near $sqrt(2\/pi) approx 0.798$, which is the mean of a
half-normal distribution and exactly what the central limit theorem predicts for
$E|S_n| \/ sqrt(n)$.

= Twenty walks at once

```r
n <- 500
par(mar = c(4, 4, 1, 1))
plot(NA, xlim = c(0, n), ylim = c(-70, 70), xlab = "steps", ylab = "position")
for (i in 1:20) lines(0:n, c(0, walk(n)), col = "#3757d533", lwd = 1.5)
curve( sqrt(x), 0, n, add = TRUE, lwd = 2, col = "#c0392b")
curve(-sqrt(x), 0, n, add = TRUE, lwd = 2, col = "#c0392b")
```

The envelope is $plus.minus sqrt(n)$. Most paths stay inside it most of the
time, and the ones that wander outside are not anomalies: they are the tail the
$sqrt(n)$ scale is a summary of.

= The arcsine law

Here is the result that makes random walks worth teaching. Ask what fraction of
its time a walk spends on the positive side. Intuition says a half, with most
walks near a half. Intuition is wrong, and not slightly.

```r
positive_fraction <- function(n) mean(walk(n) > 0)
fractions <- replicate(4000, positive_fraction(1000))

par(mar = c(4, 4, 1, 1))
hist(fractions, breaks = 40, freq = FALSE, col = "#3757d533", border = NA,
     main = "", xlab = "fraction of time spent above zero")
curve(1 / (pi * sqrt(x * (1 - x))), 0.001, 0.999, add = TRUE, lwd = 2, col = "#c0392b")
```

The density is U-shaped: $f(x) = 1 \/ (pi sqrt(x(1-x)))$. The _least_ likely
outcome is an even split. The most likely outcomes are that the walk spends
almost all of its time on one side.

The reason is that a walk which drifts positive early has to return to zero
before it can accumulate negative time, and returns to zero become rarer as the
walk wanders. Leads are sticky. In a season of coin flips, one team leading
throughout is not evidence of anything.

= Recurrence, and its price

In one dimension the walk returns to the origin with probability one. It also
takes its time about it: the expected waiting time is infinite. Both statements
hold at once, and the second is why simulating the first is awkward.

```r
return_time <- function(cap = 10000) {
  position <- 0
  for (t in seq_len(cap)) {
    position <- position + sample(c(-1, 1), 1)
    if (position == 0) return(t)
  }
  NA_integer_
}

times <- replicate(3000, return_time())
c(
  returned_within_cap = mean(!is.na(times)),
  median_time         = median(times, na.rm = TRUE),
  mean_time           = mean(times, na.rm = TRUE)
)
```

The median return is quick, a handful of steps. The mean is enormous and grows
with whatever cap you impose, because the distribution has tail
$P(T > t) tilde.op sqrt(2 \/ (pi t))$ and no finite first moment. Reporting the
mean of this sample would be reporting a property of the cap.
