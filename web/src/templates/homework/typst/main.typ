// Problem set with solutions. Set show-solutions to false to hide the
// solutions and print a blank assignment.
#import "@preview/examy:1.0.0": *

#let show-solutions = true
#let title = "{{title}}"
#let author = "{{author}}"

#set document(title: title, author: author)
#set page(
  paper: "us-letter",
  margin: (x: 1in, top: 1in, bottom: 1in),
  header: text(size: 9pt)[Course 101 #h(1fr) Problem Set 3 #h(1fr) Due: Friday, 11:59 pm],
  footer: context align(center, text(size: 9pt, counter(page).display("1 / 1", both: true))),
)
#set text(font: "Libertinus Serif", size: 11pt)
#show math.equation: set text(font: "New Computer Modern Math")

#show: exam.with(show-solutions: show-solutions)

#align(center)[
  #text(size: 16pt, weight: "bold", title)\
  Student: #author
]

#v(0.5em)

// Points per question and part, with totals for the whole assignment.
#align(center, points-table())

#v(0.5em)

#question(points: 10)[
  Let $f(x) = x^3 - 6x^2 + 9x$.
  #part(points: 4)[
    Find the critical points of $f$.
    #solution[
      $f'(x) = 3x^2 - 12x + 9 = 3(x - 1)(x - 3)$, so the critical points are $x = 1$ and $x = 3$.
    ]
  ]
  #part(points: 3)[
    Classify each critical point as a local maximum or a local minimum.
    #solution[
      $f''(x) = 6x - 12$. Since $f''(1) = -6 < 0$, $x = 1$ is a local maximum.
      Since $f''(3) = 6 > 0$, $x = 3$ is a local minimum.
    ]
  ]
  #part(points: 3)[
    Explain why $f$ has no absolute maximum on the whole real line.
    #solution[
      As $x -> infinity$, $f(x) -> infinity$, so $f$ is unbounded above and no
      value of $f$ is largest.
    ]
  ]
]

// Multiple choice: the + marks the answer shown in the solutions.
#question(points: 5)[
  Which statement about the sample variance $s^2$ is correct? _Select one._
  #choices(columns: 1)[
    - It divides the sum of squared deviations by $n$.
    + It divides the sum of squared deviations by $n - 1$, which makes it an unbiased estimator of $sigma^2$.
    - It is unaffected by a single extreme value.
    - It is always negative.
  ]
  #solution[
    The denominator $n - 1$ corrects for the fact that the deviations are
    measured from the sample mean, which uses up one degree of freedom.
  ]
]

#question(points: 12)[
  Calls to a support line arrive as a Poisson process at a rate of 3 per minute.
  Let $X$ be the number of calls in one minute.
  #part(points: 4)[
    Compute $P(X = 0)$.
    #solution[
      $P(X = 0) = e^(-3) approx 0.0498$.
    ]
  ]
  #part(points: 4)[
    Compute $P(X >= 2)$.
    #solution[
      $P(X >= 2) = 1 - P(X = 0) - P(X = 1) = 1 - 4 e^(-3) approx 0.801$.
    ]
  ]
  #part(points: 4)[
    Find the mean and the variance of the number of calls in a two-minute interval.
    #solution[
      The count in two minutes is Poisson with mean $6$, so the mean and the
      variance are both $6$.
    ]
  ]
]

#question(points: 10)[
  Prove that the sum of two even integers is even. Write your proof in full sentences.
  #solution[
    Let $a = 2m$ and $b = 2n$ for integers $m$ and $n$. Then
    $a + b = 2(m + n)$, which is twice the integer $m + n$, so $a + b$ is even.
  ]
]

// Bonus points are counted separately from the regular total.
#question(points: 5, intent: "bonus")[
  For a Poisson random variable $X$ with mean $lambda$, show that $E[X] = lambda$.
  #solution[
    $ E[X] = sum_(k=0)^infinity k e^(-lambda) lambda^k / k!
      = lambda e^(-lambda) sum_(k=1)^infinity lambda^(k-1) / (k-1)!
      = lambda e^(-lambda) e^lambda = lambda. $
  ]
]
