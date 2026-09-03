#import "/.calepin/calepin.typ" as calepin
#show: calepin.document

#set document(title: [Newton's Method Is Not Always Your Friend])
#calepin.setup(echo: true, eval: true, results: "verbatim", fenced-chunks: true)

#title()

Newton's method converges quadratically, which is the fact everyone remembers,
and it converges quadratically _near a simple root_, which is the qualification
everyone forgets. This note is about the qualification.

= The iteration

Given $f$ and a current guess $x_n$, take the tangent at $x_n$ and follow it to
the axis:

$ x_(n+1) = x_n - f(x_n) / f'(x_n) $

If $f(r) = 0$ and $f'(r) != 0$, and $x_0$ is close enough to $r$, then the error
$e_n = x_n - r$ obeys

$ e_(n+1) = (f''(xi_n)) / (2 f'(x_n)) e_n^2 $

for some $xi_n$ between $x_n$ and $r$. The square is where the speed comes from:
the number of correct digits roughly doubles each step.

= When it behaves

```r
newton <- function(f, fp, x0, steps = 8) {
  x <- numeric(steps + 1)
  x[1] <- x0
  for (i in seq_len(steps)) x[i + 1] <- x[i] - f(x[i]) / fp(x[i])
  x
}

path <- newton(function(x) x^2 - 2, function(x) 2 * x, x0 = 1)
data.frame(step = 0:8, x = path, error = abs(path - sqrt(2)))
```

Eight steps take a poor initial guess to the limit of double precision. The
error column is the interesting one: each entry is roughly the square of the one
above it, until there is nothing left to halve.

= Three ways it fails

The quadratic rate assumes a simple root, a nonzero derivative, and a starting
point in the basin of attraction. Drop any of the three and the method
misbehaves in its own characteristic way.

== A repeated root

At a double root $f'(r) = 0$ too, and the tangent is nearly flat where we most
need it to be steep. Convergence degrades from quadratic to linear, with the
error merely halving each step.

```r
simple   <- newton(function(x) x^2 - 2,     function(x) 2 * x,           x0 = 2, steps = 12)
repeated <- newton(function(x) (x - 2)^2,   function(x) 2 * (x - 2),     x0 = 3, steps = 12)

par(mar = c(4, 4, 1, 1))
plot(0:12, abs(simple - sqrt(2)) + 1e-18, type = "b", log = "y", pch = 19,
     lwd = 2, col = "#3757d5", xlab = "step", ylab = "absolute error")
lines(0:12, abs(repeated - 2) + 1e-18, type = "b", pch = 19, lwd = 2, col = "#c0392b")
legend("bottomleft", bty = "n", lwd = 2, col = c("#3757d5", "#c0392b"),
       legend = c("simple root: quadratic", "double root: linear"))
```

On a logarithmic scale, quadratic convergence is a curve that falls away from
the straight line of linear convergence. The blue path reaches machine precision
in six steps; the red one is still crawling at twelve.

== A cycle

Nothing forbids the iteration from returning to where it started. For
$f(x) = x^3 - 2x + 2$ the point $x_0 = 0$ maps to $1$, and $1$ maps back to $0$,
forever.

```r
f  <- function(x) x^3 - 2 * x + 2
fp <- function(x) 3 * x^2 - 2
round(newton(f, fp, x0 = 0, steps = 6), 12)
```

The method is not lost, and it is not diverging. It is on a stable two-cycle,
and it will sit there for as long as you let it run.

== Sensitivity to the start

Where a cubic has three roots, the map from starting point to eventual root is
not a tidy partition into three intervals. It is intricate on the real line and
famously fractal in the complex plane.

```r
roots <- c(-1, 0, 1)
starts <- seq(-1.2, 1.2, length.out = 1200)
found <- vapply(starts, function(s) {
  x <- s
  for (i in 1:60) {
    d <- 3 * x^2 - 1
    if (abs(d) < 1e-12) return(NA_real_)
    x <- x - (x^3 - x) / d
  }
  roots[which.min(abs(x - roots))]
}, numeric(1))

par(mar = c(4, 4, 1, 1))
plot(starts, found, type = "p", pch = 15, cex = 0.35,
     col = c("#c0392b", "#17202a", "#3757d5")[match(found, roots)],
     xlab = expression(x[0]), ylab = "root reached", yaxt = "n")
axis(2, at = roots, labels = c("-1", "0", "1"))
```

Between about $-0.6$ and $0.6$ the outcome flips repeatedly over vanishingly
small changes in the start. Two initial guesses agreeing to three decimals can
land on different roots.

= What to do instead

Newton is a local method wearing a global disguise. In practice one either
brackets the root first, or damps the step:

$ x_(n+1) = x_n - lambda_n f(x_n) / f'(x_n), quad lambda_n in (0, 1] $

choosing $lambda_n$ to guarantee that $|f|$ actually decreases. That single
safeguard converts a method that can cycle forever into one that cannot, at the
cost of a few extra evaluations near the solution.
