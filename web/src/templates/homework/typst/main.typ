#let problem(points, body) = {
  set block(spacing: 1em)
  [
    *Problem* (#points points)
    #body
  ]
}

#set document(title: "{{title}}", author: "{{author}}")

#align(center, text(size: 18pt, weight: "bold", [{{title}}]))

#align(center, text(size: 12pt, [{{author}}]))

#problem(10, [
  Solve the following equation:
  $ x^2 + 3x + 2 = 0 $

  (a) Find the roots of this quadratic equation.

  (b) Verify your solutions by substituting back into the original equation.

  _Space for answer:_
  #v(3em)
])

#problem(8, [
  Calculate the derivative of the function:
  $ f(x) = 3x^3 - 2x^2 + x - 5 $

  Show your work step by step.

  _Space for answer:_
  #v(2.5em)
])

#problem(7, [
  Evaluate the definite integral:
  $ integral_0^2 (2x + 1) dif x $

  (a) Set up the integral.

  (b) Compute the antiderivative.

  (c) Evaluate using the fundamental theorem of calculus.

  _Space for answer:_
  #v(3em)
])

#problem(5, [
  Multiple choice: Which of the following is true?

  (a) All prime numbers are odd.

  (b) The sum of any two even numbers is even.

  (c) Every positive integer is a prime number.

  (d) Zero is a positive number.

  _Space for answer:_
  #v(1.5em)
])
